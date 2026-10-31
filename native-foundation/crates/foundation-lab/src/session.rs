//! Bounded interactive lab controller, independent from any window or input backend.
use crate::{
    factory, quest_factory,
    world_storage::{Backend, WorldStore},
};
use industrial_sim::{
    content::{Catalog as Items, Id},
    inventory::{Inventory, Limits as PortLimits},
    machine::Machine,
};
use quest_core::catalog::{Catalog as Quests, QuestId};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use voxel_core::{
    coords::CubePos,
    storage::CubeStorage,
    streaming::{Limits, Streamer, Tickets},
};
pub type Error = Box<dyn std::error::Error>;
const OWNER: &str = "native-foundation-viewer-v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    schema_version: u32,
    owner: String,
    backend: String,
    content_version: u32,
}
#[derive(Clone, Debug, Serialize)]
pub struct MachineStatus {
    pub completed: u64,
    pub progress: Option<u32>,
    pub energy: u64,
    pub loaded: bool,
    pub simulated: bool,
    pub inputs: Vec<(String, u64)>,
    pub outputs: Vec<(String, u64)>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub machines: Vec<MachineStatus>,
    pub submitted: u64,
    pub reward_tokens: u64,
    pub reward_claimed: bool,
    pub material_units: u64,
    pub energy_used: u64,
    pub resident_cubes: usize,
    pub charged_cube_bytes: usize,
    pub focus: [i64; 3],
}
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Tick {
    pub machines: usize,
    pub energy_used: u64,
    pub dust_transferred: u64,
}
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Submission {
    pub items: u64,
    pub reward_tokens: u64,
}
pub struct Session {
    root: PathBuf,
    backend: Backend,
    items: Items,
    quests: Quests,
    stream: Streamer<WorldStore>,
    focus: CubePos,
}
impl Session {
    pub fn new(backend: Backend) -> Result<Self, Error> {
        let root = factory::world_root(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
        let items = factory::catalog()?;
        let quests = Quests::from_json(include_bytes!("../../../fixtures/quests-v1.json"))?;
        let mut store = WorldStore::open(&root, backend)?;
        let mut records = factory::initial_records(&items)?;
        records.extend(quest_factory::initial_records(&quests)?);
        store.save_batch(&records.iter().collect::<Vec<_>>())?;
        let metadata = Metadata {
            schema_version: 1,
            owner: OWNER.into(),
            backend: backend.name().into(),
            content_version: 1,
        };
        let mut marker = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("viewer-owned.json"))?;
        marker.write_all(&serde_json::to_vec_pretty(&metadata)?)?;
        marker.sync_all()?;
        Self::from_store(root, backend, items, quests, store)
    }
    pub fn resume(root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = root.as_ref().canonicalize()?;
        let base = factory::worlds_base()?;
        if root == base || !root.starts_with(&base) {
            return Err("viewer resumes only its task-private worlds".into());
        }
        let marker = root.join("viewer-owned.json");
        let metadata = fs::symlink_metadata(&marker)?;
        if !metadata.file_type().is_file() || metadata.len() > 4096 {
            return Err("invalid viewer ownership marker".into());
        }
        let marker: Metadata = serde_json::from_slice(&fs::read(marker)?)?;
        if marker.schema_version != 1 || marker.owner != OWNER || marker.content_version != 1 {
            return Err("unknown viewer world schema; files preserved".into());
        }
        let backend = Backend::from_name(&marker.backend)?;
        let required = match backend {
            Backend::Journal => ".world-format",
            #[cfg(feature = "paged")]
            Backend::Paged => "native-world-v1.redb",
        };
        if !fs::symlink_metadata(root.join(required))?
            .file_type()
            .is_file()
        {
            return Err("owned world storage missing; no replacement created".into());
        }
        let items = factory::catalog()?;
        let quests = Quests::from_json(include_bytes!("../../../fixtures/quests-v1.json"))?;
        let store = WorldStore::open(&root, backend)?;
        Self::from_store(root, backend, items, quests, store)
    }
    fn from_store(
        root: PathBuf,
        backend: Backend,
        items: Items,
        quests: Quests,
        mut store: WorldStore,
    ) -> Result<Self, Error> {
        for pos in quest_factory::PARTICIPANTS {
            if store.load(pos)?.is_none() {
                return Err("owned lab participant missing; files preserved".into());
            }
        }
        let stream = Streamer::new(
            store,
            Limits {
                max_cubes: 27,
                max_resident_bytes: 27 * 32768,
                max_cube_bytes: 32768,
                max_pending_jobs: 4,
            },
        )?;
        let mut session = Self {
            root,
            backend,
            items,
            quests,
            stream,
            focus: quest_factory::CRUSHER,
        };
        session.set_focus(quest_factory::CRUSHER)?;
        session.status()?;
        Ok(session)
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn backend(&self) -> Backend {
        self.backend
    }
    pub fn world(&self) -> &Streamer<WorldStore> {
        &self.stream
    }
    pub fn focus(&self) -> CubePos {
        self.focus
    }
    pub fn set_focus(&mut self, focus: CubePos) -> Result<(), Error> {
        let tickets = Tickets::around(focus, [1; 3], [1; 3])?;
        for job in self.stream.set_tickets(tickets) {
            if !self.stream.acknowledge_canceled(job) {
                return Err("canceled lab job acknowledgment failed".into());
            }
        }
        self.focus = focus;
        Ok(())
    }
    pub fn pump(&mut self) -> Result<usize, Error> {
        let plan = self.stream.plan(4, 27)?;
        let loaded = plan.loads.len();
        for job in plan.loads {
            let cube = self.stream.read_or_generate(job, factory::generated)?;
            if !self.stream.complete(job, cube)? {
                return Err("current lab load discarded".into());
            }
        }
        Ok(loaded)
    }
    pub fn tick(&mut self) -> Result<Tick, Error> {
        let active: Vec<_> = factory::MACHINE_POSITIONS
            .into_iter()
            .filter(|p| self.stream.simulates(*p))
            .collect();
        if active.is_empty() {
            return Ok(Tick {
                machines: 0,
                energy_used: 0,
                dust_transferred: 0,
            });
        }
        let items = &self.items;
        let (energy_used, dust_transferred) = self
            .stream
            .transact(&active, |r| factory::advance(r, items))?;
        Ok(Tick {
            machines: active.len(),
            energy_used,
            dust_transferred,
        })
    }
    pub fn submit(&mut self) -> Result<Submission, Error> {
        if quest_factory::PARTICIPANTS
            .iter()
            .any(|p| !self.stream.simulates(*p))
        {
            return Err(
                "return to the lab and wait for its four cubes to load before submitting".into(),
            );
        }
        let before = self.status()?.submitted;
        let (items, quests) = (&self.items, &self.quests);
        let tokens = self
            .stream
            .transact(&quest_factory::PARTICIPANTS, |records| {
                quest_factory::submit(records, items, quests)
            })?;
        Ok(Submission {
            items: self.status()?.submitted - before,
            reward_tokens: tokens,
        })
    }
    fn payload(&mut self, pos: CubePos) -> Result<Vec<u8>, Error> {
        if let Some(cube) = self.stream.record(pos) {
            return Ok(cube.payload().to_vec());
        }
        Ok(self
            .stream
            .storage_mut()
            .load(pos)?
            .ok_or("lab participant data missing")?
            .payload()
            .to_vec())
    }
    pub fn status(&mut self) -> Result<Status, Error> {
        let mut machines = Vec::new();
        let mut states = Vec::new();
        for pos in factory::MACHINE_POSITIONS {
            let bytes = self.payload(pos)?;
            let machine = Machine::decode(&bytes, &self.items)?;
            let ports = PortLimits {
                items: 16,
                fluid_mb: 16000,
                kinds: 8,
            };
            let expected = if pos == quest_factory::CRUSHER {
                "lab:crusher"
            } else {
                "lab:smelter"
            };
            if machine.kind().as_str() != expected
                || machine.inputs().limits() != ports
                || machine.outputs().limits() != ports
                || machine.energy().capacity() != 4096
                || machine.energy().max_voltage() != 32
                || machine.completed() > 12
            {
                return Err(
                    "machine definition does not match this synthetic lab; files preserved".into(),
                );
            }
            machines.push(MachineStatus {
                completed: machine.completed(),
                progress: machine.progress(),
                energy: machine.energy().stored(),
                loaded: self.stream.record(pos).is_some(),
                simulated: self.stream.simulates(pos),
                inputs: machine
                    .inputs()
                    .contents()
                    .map(|(id, n)| (id.to_string(), n))
                    .collect(),
                outputs: machine
                    .outputs()
                    .contents()
                    .map(|(id, n)| (id.to_string(), n))
                    .collect(),
            });
            states.push(machine);
        }
        let ledger = self.payload(quest_factory::LEDGER)?;
        let (book, submitted) = quest_factory::decode_ledger(&ledger, &self.items, &self.quests)?;
        let wallet = self.payload(quest_factory::PLAYER)?;
        let wallet: Inventory = serde_json::from_slice(&wallet)?;
        wallet.validate(&self.items)?;
        let ingot = Id::new("lab:iron_ingot")?;
        let token = Id::new("lab:reward_token")?;
        if submitted.limits()
            != (PortLimits {
                items: 16,
                fluid_mb: 0,
                kinds: 2,
            })
            || wallet.limits()
                != (PortLimits {
                    items: 4,
                    fluid_mb: 0,
                    kinds: 1,
                })
        {
            return Err("quest/reward port definition changed; files preserved".into());
        }
        if submitted.contents().any(|(id, _)| id != &ingot)
            || wallet.contents().any(|(id, _)| id != &token)
        {
            return Err("unsupported resource in lab quest/reward ports".into());
        }
        let submitted = submitted.amount(&ingot);
        let claimed = book.has_claimed(&QuestId::from_u128(2));
        if submitted > 12
            || book.task_progress(&QuestId::from_u128(2), 0) != submitted
            || book.task_progress(&QuestId::from_u128(1), 0) > states[0].completed().min(1)
            || wallet.amount(&token) != if claimed { 2 } else { 0 }
        {
            return Err("quest/reward ledger mismatch; files preserved".into());
        }
        let material_units = factory::mass(&states) + submitted * 2;
        if material_units != 24 {
            return Err("lab material ledger mismatch; files preserved".into());
        }
        let energy = states.iter().map(|m| m.energy().stored()).sum::<u64>();
        let energy_used = 8192u64
            .checked_sub(energy)
            .ok_or("lab energy ledger mismatch")?;
        Ok(Status {
            machines,
            submitted,
            reward_tokens: wallet.amount(&token),
            reward_claimed: claimed,
            material_units,
            energy_used,
            resident_cubes: self.stream.resident_positions().count(),
            charged_cube_bytes: self.stream.charged_bytes(),
            focus: [self.focus.x, self.focus.y, self.focus.z],
        })
    }
    pub fn save(&mut self) -> Result<(), Error> {
        self.stream.flush_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fill(session: &mut Session) {
        for _ in 0..16 {
            session.pump().unwrap();
        }
    }
    #[test]
    fn interactive_ticks_submissions_and_restart_preserve_the_factory_and_single_reward() {
        let mut session = Session::new(Backend::Journal).unwrap();
        fill(&mut session);
        assert_eq!(session.status().unwrap().material_units, 24);
        for _ in 0..100 {
            session.tick().unwrap();
        }
        let status = session.status().unwrap();
        assert_eq!(status.machines[1].completed, 12);
        assert_eq!(status.energy_used, 1824);
        for _ in 0..6 {
            assert_eq!(session.submit().unwrap().items, 1);
        }
        let root = session.root().to_path_buf();
        session.save().unwrap();
        drop(session);
        let mut session = Session::resume(&root).unwrap();
        fill(&mut session);
        assert_eq!(session.status().unwrap().submitted, 6);
        for _ in 0..6 {
            session.submit().unwrap();
        }
        for _ in 0..12 {
            assert_eq!(session.submit().unwrap().reward_tokens, 0);
        }
        let status = session.status().unwrap();
        assert_eq!(status.material_units, 24);
        assert_eq!(status.submitted, 12);
        assert_eq!(status.reward_tokens, 2);
        assert!(status.reward_claimed);
        session.save().unwrap();
        drop(session);
        let mut session = Session::resume(&root).unwrap();
        assert_eq!(session.status().unwrap().reward_tokens, 2);
    }
    #[test]
    fn leaving_simulation_interest_pauses_processing_and_rejects_remote_submissions() {
        let mut session = Session::new(Backend::Journal).unwrap();
        fill(&mut session);
        session.tick().unwrap();
        let before = session.status().unwrap();
        session.set_focus(CubePos::new(-1, -1_000_000, 0)).unwrap();
        fill(&mut session);
        for _ in 0..20 {
            assert_eq!(session.tick().unwrap().machines, 0);
        }
        assert!(session.submit().is_err());
        let after = session.status().unwrap();
        assert_eq!(after.machines[0].progress, before.machines[0].progress);
        assert_eq!(after.energy_used, before.energy_used);
        assert!(after.machines.iter().all(|m| !m.loaded && !m.simulated));
        assert!(after.resident_cubes <= 27);
    }
    #[test]
    fn resume_rejects_unowned_paths_and_conflicting_owners_without_repairing_them() {
        assert!(Session::resume(Path::new(env!("CARGO_MANIFEST_DIR"))).is_err());
        let mut session = Session::new(Backend::Journal).unwrap();
        let root = session.root().to_path_buf();
        assert!(Session::resume(&root).is_err());
        session.save().unwrap();
        drop(session);
        let marker = root.join("viewer-owned.json");
        let mut data: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
        data["schema_version"] = serde_json::json!(99);
        let bytes = serde_json::to_vec(&data).unwrap();
        fs::write(&marker, &bytes).unwrap();
        assert!(Session::resume(&root).is_err());
        assert_eq!(fs::read(marker).unwrap(), bytes);
    }
    #[cfg(feature = "paged")]
    #[test]
    fn paged_session_runs_the_same_interactive_submission_flow() {
        let mut session = Session::new(Backend::Paged).unwrap();
        fill(&mut session);
        for _ in 0..100 {
            session.tick().unwrap();
        }
        for _ in 0..12 {
            session.submit().unwrap();
        }
        let status = session.status().unwrap();
        assert_eq!(status.material_units, 24);
        assert_eq!(status.reward_tokens, 2);
    }

    #[test]
    fn altered_machine_capacity_cannot_bypass_the_lab_ledger_or_panic_on_resume() {
        let session = Session::new(Backend::Journal).unwrap();
        let root = session.root().to_path_buf();
        drop(session);
        let mut store = WorldStore::open(&root, Backend::Journal).unwrap();
        let mut cube = store.load(quest_factory::CRUSHER).unwrap().unwrap();
        let mut data: serde_json::Value = serde_json::from_slice(cube.payload()).unwrap();
        data["inputs"]["limits"]["items"] = serde_json::json!(u64::MAX);
        data["inputs"]["contents"]["lab:iron_ore"] = serde_json::json!(u64::MAX);
        cube.replace_payload(serde_json::to_vec(&data).unwrap())
            .unwrap();
        store.save(&cube).unwrap();
        drop(store);
        assert!(Session::resume(&root).is_err());
    }

    #[test]
    fn a_reward_port_cannot_restore_tokens_without_a_matching_claim() {
        let session = Session::new(Backend::Journal).unwrap();
        let root = session.root().to_path_buf();
        drop(session);
        let mut store = WorldStore::open(&root, Backend::Journal).unwrap();
        let mut cube = store.load(quest_factory::PLAYER).unwrap().unwrap();
        let mut data: serde_json::Value = serde_json::from_slice(cube.payload()).unwrap();
        data["contents"]["lab:reward_token"] = serde_json::json!(2);
        cube.replace_payload(serde_json::to_vec(&data).unwrap())
            .unwrap();
        store.save(&cube).unwrap();
        drop(store);
        assert!(Session::resume(&root).is_err());
    }
}
