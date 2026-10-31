//! Executable architecture checks. Panicking assertions are failed checks, never PASS.

#[cfg(feature = "paged")]
use foundation_lab::storage_bench;
use foundation_lab::{
    asset_stress, cube_stress, factory, journal_probe, quest_factory, world_storage,
};
#[cfg(feature = "gpu")]
use foundation_lab::{gpu_assets, world_scene};

use serde_json::json;
use std::{env, fs, path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let scenario = args.first().map(String::as_str).unwrap_or("core");
    #[cfg(feature = "paged")]
    if scenario == "storage-bench" {
        return storage_bench::execute(&args[1..]);
    }
    if scenario == "journal-probe" {
        return journal_probe::execute(&args[1..]);
    }
    if scenario == "automation-probe" {
        return foundation_lab::automated_factory::interruption::execute(&args[1..]);
    }
    if scenario == "recipe-probe" {
        return foundation_lab::recipe_bench::execute(&args[1..]);
    }
    let output = PathBuf::from(
        args.get(1)
            .map(String::as_str)
            .unwrap_or("../evidence/foundation-core.json"),
    );
    let start = Instant::now();
    let mut report = json!({"scenario":scenario,"version":1,"status":"RUNNING","cpu_payloads":"actual allocated buffers","gpu_accounting":"modeled in this core scenario; real GPU verification is separate"});
    match scenario {
        "recipe-reference" => {
            report["recipe_reference"] = foundation_lab::recipe_reference::run()?;
        }
        "collision-resident" => {
            report["gpu_accounting"] = json!("not used in this readonly collision scenario");
            report["collision"] = foundation_lab::collision_scene::run()?;
        }
        "retrieval-world" => {
            report["retrieval"] = foundation_lab::retrieval_world::run(
                std::path::Path::new(
                    args.get(2)
                        .ok_or("retrieval-world requires private profile")?,
                ),
                world_storage::Backend::Journal,
            )?
        }
        #[cfg(feature = "paged")]
        "retrieval-world-paged" => {
            report["retrieval"] = foundation_lab::retrieval_world::run(
                std::path::Path::new(
                    args.get(2)
                        .ok_or("retrieval-world-paged requires private profile")?,
                ),
                world_storage::Backend::Paged,
            )?
        }
        "retrieval-counts" => {
            report["retrieval_counts"] =
                foundation_lab::retrieval_oracle::run(std::path::Path::new(
                    args.get(2)
                        .ok_or("retrieval-counts requires reference cases")?,
                ))?
        }
        "legacy-items" => {
            report["legacy_items"] = foundation_lab::item_oracle::run(std::path::Path::new(
                args.get(2).ok_or("legacy-items requires reference cases")?,
            ))?
        }
        #[cfg(feature = "gpu")]
        "dense-mesh" => {
            report["gpu_accounting"] = json!(
                "actual GPU mesh allocations; completion-fenced retirement, fixed targets separate"
            );
            report["dense_mesh"] = foundation_lab::mesh_scene::run(&output)?;
        }
        "logistics" => {
            report["logistics"] = foundation_lab::logistics::run(world_storage::Backend::Journal)?
        }
        "automated-factory" => {
            report["automation"] =
                foundation_lab::automated_factory::run(world_storage::Backend::Journal)?;
        }
        #[cfg(feature = "paged")]
        "automated-factory-paged" => {
            report["automation"] =
                foundation_lab::automated_factory::run(world_storage::Backend::Paged)?;
        }
        #[cfg(feature = "paged")]
        "logistics-paged" => {
            report["logistics"] = foundation_lab::logistics::run(world_storage::Backend::Paged)?
        }
        "quest-gates" => {
            report["gpu_accounting"] = json!("not used in this reference comparison");
            let reference = args
                .get(2)
                .ok_or("quest-gates requires a reference file as the third argument")?;
            report["quest_gates"] =
                foundation_lab::quest_oracle::run(std::path::Path::new(reference))?;
        }
        "core" => {
            report["assets"] = asset_stress::run(0x1234_5678, 100_000, 10_000);
            report["cubes"] = cube_stress::run(120)?;
        }
        "assets" => report["assets"] = asset_stress::run(0x1234_5678, 100_000, 10_000),
        "cubes" => report["cubes"] = cube_stress::run(120)?,
        "factory" => {
            let outcome = factory::run(128)?;
            assert_eq!(outcome.final_state.len(), 2);
            report["factory"] = outcome.report;
        }
        "quest-factory" => report["quests"] = quest_factory::run(world_storage::Backend::Journal)?,
        #[cfg(feature = "paged")]
        "quest-factory-paged" => {
            report["quests"] = quest_factory::run(world_storage::Backend::Paged)?
        }
        #[cfg(feature = "paged")]
        "factory-paged" => {
            let outcome =
                factory::run_with_backend(128, world_storage::Backend::Paged, |_, _, _| Ok(()))?;
            assert_eq!(outcome.final_state.len(), 2);
            report["factory"] = outcome.report;
        }
        #[cfg(feature = "gpu")]
        "gpu-assets" => {
            report["gpu_accounting"] =
                json!("actual wgpu textures, with backend retirement fences");
            report["gpu_assets"] = gpu_assets::run(&output)?;
        }
        #[cfg(feature = "gpu")]
        "factory-gpu" => {
            report["gpu_accounting"] = json!(
                "actual cube vertex buffers with completion-fenced retirement; fixed render resources reported separately"
            );
            let (factory, render) = world_scene::run(&output, world_storage::Backend::Journal)?;
            report["factory"] = factory;
            report["render"] = render;
        }
        #[cfg(all(feature = "paged", feature = "gpu"))]
        "factory-paged-gpu" => {
            report["gpu_accounting"] = json!(
                "actual cube vertex buffers with completion-fenced retirement; fixed render resources reported separately"
            );
            let (factory, render) = world_scene::run(&output, world_storage::Backend::Paged)?;
            report["factory"] = factory;
            report["render"] = render;
        }
        _ => return Err(format!("unknown scenario: {scenario}").into()),
    }
    report["status"] = json!("PASS");
    report["seconds"] = json!(start.elapsed().as_secs_f64());
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
