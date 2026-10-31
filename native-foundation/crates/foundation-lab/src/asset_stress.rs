use residency::{
    AssetId, Budget, Cache, CacheError, Handle, LoadToken, Priority, Published, Request,
    RetireToken, Spec, Usage,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

struct Body {
    cpu: Vec<u8>,
    transient: Vec<u8>,
    gpu: u64,
}
struct Loading {
    token: LoadToken,
    body: Body,
    due: u64,
}
struct Retiring {
    token: RetireToken,
    body: Body,
    due: u64,
}

struct Backend {
    ready: BTreeMap<Handle, Body>,
    loading: Vec<Loading>,
    retiring: Vec<Retiring>,
    pins: Vec<(Handle, u64)>,
    peak_actual_cpu: u64,
    assertions: u64,
}

impl Backend {
    fn new() -> Self {
        Self {
            ready: BTreeMap::new(),
            loading: Vec::new(),
            retiring: Vec::new(),
            pins: Vec::new(),
            peak_actual_cpu: 0,
            assertions: 0,
        }
    }
    fn finish(&mut self, cache: &mut Cache, step: u64) {
        let mut pending = Vec::new();
        for (handle, due) in self.pins.drain(..) {
            if due <= step {
                cache
                    .unpin(handle)
                    .expect("a pinned asset cannot be evicted");
            } else {
                pending.push((handle, due));
            }
        }
        self.pins = pending;
        let mut loading = Vec::new();
        for mut item in self.loading.drain(..) {
            if item.due > step {
                loading.push(item);
                continue;
            }
            item.body.transient = Vec::new();
            match cache
                .publish(item.token, item.body.cpu.len() as u64, item.body.gpu)
                .expect("valid completion")
            {
                Published::Ready(handle) => {
                    assert!(self.ready.insert(handle, item.body).is_none());
                }
                Published::Retire(token) => self.retiring.push(Retiring {
                    token,
                    body: item.body,
                    due: step + 2,
                }),
            }
        }
        self.loading = loading;
        let mut retiring = Vec::new();
        for item in self.retiring.drain(..) {
            if item.due > step {
                retiring.push(item);
                continue;
            }
            let token = item.token;
            drop(item.body);
            assert!(cache.acknowledge_retired(token));
        }
        self.retiring = retiring;
    }
    fn request(
        &mut self,
        cache: &mut Cache,
        id: AssetId,
        priority: Priority,
        step: u64,
    ) -> Option<Handle> {
        match cache
            .request(id, priority)
            .expect("registered fitting asset")
        {
            Request::Ready(handle) => {
                assert!(self.ready.contains_key(&handle));
                Some(handle)
            }
            Request::Load(token) => {
                let spec = cache.spec(id).unwrap();
                let body = Body {
                    cpu: vec![(id.0 % 251) as u8; spec.cpu_bytes as usize],
                    transient: vec![0; spec.transient_cpu_bytes as usize],
                    gpu: spec.gpu_bytes,
                };
                self.loading.push(Loading {
                    token,
                    body,
                    due: step + 2 + (id.0 % 5),
                });
                None
            }
            Request::Deferred { retire, .. } => {
                for token in retire {
                    let body = self
                        .ready
                        .remove(&token.0)
                        .expect("retire an actual ready allocation");
                    self.retiring.push(Retiring {
                        token,
                        body,
                        due: step + 3,
                    });
                }
                None
            }
        }
    }
    fn verify(&mut self, cache: &Cache) {
        let usage = cache.usage();
        let budget = cache.budget();
        let bodies = self
            .ready
            .values()
            .chain(self.loading.iter().map(|x| &x.body))
            .chain(self.retiring.iter().map(|x| &x.body));
        let (mut actual_cpu, mut modeled_gpu) = (0, 0);
        for body in bodies {
            actual_cpu += (body.cpu.capacity() + body.transient.capacity()) as u64;
            modeled_gpu += body.gpu;
        }
        assert!(usage.cpu() <= budget.cpu_bytes && usage.gpu() <= budget.gpu_bytes);
        assert!(usage.entries() <= budget.max_entries && usage.loading <= budget.max_loading);
        assert!(
            actual_cpu <= usage.cpu(),
            "backend CPU allocations exceed charged bytes"
        );
        assert!(
            modeled_gpu <= usage.gpu(),
            "backend resource ownership exceeds charged GPU bytes"
        );
        for handle in self.ready.keys() {
            assert!(cache.is_ready(*handle));
        }
        self.peak_actual_cpu = self.peak_actual_cpu.max(actual_cpu);
        self.assertions += 5 + self.ready.len() as u64;
    }
}

fn random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

pub fn run(seed: u64, steps: u64, catalog: u64) -> Value {
    let budget = Budget {
        cpu_bytes: 64 * 1024,
        gpu_bytes: 32 * 1024,
        max_entries: 64,
        max_loading: 12,
    };
    let mut cache = Cache::new(budget).unwrap();
    for id in 0..catalog {
        cache
            .register(Spec {
                id: AssetId(id),
                cpu_bytes: 256 + (id % 16) * 128,
                gpu_bytes: 128 + (id % 12) * 128,
                transient_cpu_bytes: 128 + (id % 4) * 64,
            })
            .unwrap();
    }
    cache
        .register(Spec {
            id: AssetId(catalog),
            cpu_bytes: budget.cpu_bytes + 1,
            gpu_bytes: 1,
            transient_cpu_bytes: 0,
        })
        .unwrap();
    let mut backend = Backend::new();
    let mut rng = seed;
    for step in 0..steps {
        backend.finish(&mut cache, step);
        let value = random(&mut rng);
        let id = if value.is_multiple_of(5) {
            value % catalog
        } else {
            value % 64
        };
        let priority = if id % 17 == 0 {
            Priority::Critical
        } else {
            Priority::Visible
        };
        if let Some(handle) = backend.request(&mut cache, AssetId(id), priority, step)
            && value.is_multiple_of(7)
        {
            cache.pin(handle).unwrap();
            backend.pins.push((handle, step + 8));
        }
        if step % 101 == 0
            && let Some(item) = backend.loading.first()
        {
            cache.cancel(item.token).unwrap();
        }
        if step % 997 == 0 {
            assert_eq!(
                cache.request(AssetId(catalog), Priority::Critical),
                Err(CacheError::TooLarge)
            );
        }
        backend.verify(&cache);
    }
    for step in steps..steps + 32 {
        backend.finish(&mut cache, step);
        backend.verify(&cache);
    }
    assert!(backend.loading.is_empty() && backend.retiring.is_empty() && backend.pins.is_empty());
    let handles: Vec<_> = cache.ready_handles().collect();
    for handle in handles {
        let token = cache.retire(handle).unwrap();
        let body = backend.ready.remove(&handle).unwrap();
        drop(body);
        assert!(cache.acknowledge_retired(token));
    }
    backend.verify(&cache);
    assert_eq!(cache.usage(), Usage::default());
    let stats = cache.stats();
    assert!(stats.admitted > 1000 && stats.evicted > 1000 && stats.canceled > 0 && stats.hits > 0);
    json!({"status":"PASS","seed":seed,"steps":steps,"catalog_entries":catalog,"assertions":backend.assertions,"requests":stats.requests,"hits":stats.hits,"loads":stats.admitted,"evictions":stats.evicted,"cancellations":stats.canceled,"deferrals":stats.deferred,"cpu_budget":budget.cpu_bytes,"gpu_budget":budget.gpu_bytes,"peak_charged_cpu":stats.peak_cpu,"peak_charged_gpu":stats.peak_gpu,"peak_actual_cpu_payload_capacity":backend.peak_actual_cpu,"peak_entries":stats.peak_entries,"ending_payload_bytes":0,"gpu_backend":"modeled only in this scenario"})
}
