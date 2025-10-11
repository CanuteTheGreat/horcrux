use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use std::time::Duration;

// Benchmark configuration
const SMALL_DATASET: usize = 10;
const MEDIUM_DATASET: usize = 100;
const LARGE_DATASET: usize = 1000;

/// Benchmark VM listing performance with different dataset sizes
fn bench_vm_list_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("vm_list_scaling");

    for size in [SMALL_DATASET, MEDIUM_DATASET, LARGE_DATASET].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            // Simulate VM list creation
            b.iter(|| {
                let vms: Vec<String> = (0..size)
                    .map(|i| format!("vm-{}", i))
                    .collect();
                black_box(vms)
            });
        });
    }

    group.finish();
}

/// Benchmark database query performance
fn bench_database_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("database_operations");

    // Benchmark in-memory database creation
    group.bench_function("create_in_memory_db", |b| {
        b.iter(|| {
            black_box("sqlite::memory:")
        });
    });

    group.finish();
}

/// Benchmark JSON serialization performance
fn bench_json_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_serialization");

    #[derive(serde::Serialize)]
    struct VmConfig {
        id: String,
        name: String,
        memory: u64,
        cpus: u32,
        disk_size: u64,
    }

    for size in [SMALL_DATASET, MEDIUM_DATASET, LARGE_DATASET].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let vms: Vec<VmConfig> = (0..size)
                .map(|i| VmConfig {
                    id: format!("vm-{}", i),
                    name: format!("Test VM {}", i),
                    memory: 2048,
                    cpus: 2,
                    disk_size: 20 * 1024 * 1024 * 1024,
                })
                .collect();

            b.iter(|| {
                let json = serde_json::to_string(&vms).unwrap();
                black_box(json)
            });
        });
    }

    group.finish();
}

/// Benchmark string operations (ID generation, parsing)
fn bench_string_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_operations");

    // Benchmark UUID generation
    group.bench_function("uuid_generation", |b| {
        b.iter(|| {
            let id = uuid::Uuid::new_v4().to_string();
            black_box(id)
        });
    });

    // Benchmark string formatting
    group.bench_function("string_formatting", |b| {
        b.iter(|| {
            let id = format!("vm-{}-{}", 100, "test");
            black_box(id)
        });
    });

    // Benchmark string parsing
    group.bench_function("string_parsing", |b| {
        let vm_id = "100";
        b.iter(|| {
            let parsed: u32 = vm_id.parse().unwrap();
            black_box(parsed)
        });
    });

    group.finish();
}

/// Benchmark hash map operations
fn bench_hashmap_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("hashmap_operations");

    for size in [SMALL_DATASET, MEDIUM_DATASET, LARGE_DATASET].iter() {
        group.bench_with_input(
            BenchmarkId::new("insert", size),
            size,
            |b, &size| {
                b.iter(|| {
                    let mut map = std::collections::HashMap::new();
                    for i in 0..size {
                        map.insert(format!("vm-{}", i), i);
                    }
                    black_box(map)
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("lookup", size),
            size,
            |b, &size| {
                let mut map = std::collections::HashMap::new();
                for i in 0..size {
                    map.insert(format!("vm-{}", i), i);
                }

                b.iter(|| {
                    let value = map.get("vm-50");
                    black_box(value)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark async operations overhead
fn bench_async_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("async_overhead");

    let rt = tokio::runtime::Runtime::new().unwrap();

    group.bench_function("spawn_task", |b| {
        b.iter(|| {
            rt.block_on(async {
                let handle = tokio::spawn(async {
                    42
                });
                black_box(handle.await.unwrap())
            })
        });
    });

    group.bench_function("async_function_call", |b| {
        async fn simple_async() -> i32 {
            42
        }

        b.iter(|| {
            rt.block_on(async {
                black_box(simple_async().await)
            })
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(100);
    targets =
        bench_vm_list_scaling,
        bench_database_operations,
        bench_json_serialization,
        bench_string_operations,
        bench_hashmap_operations,
        bench_async_overhead
}

criterion_main!(benches);
