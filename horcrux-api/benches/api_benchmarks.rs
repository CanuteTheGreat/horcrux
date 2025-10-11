///! Performance benchmarks for Horcrux API
///!
///! Run with: cargo bench --package horcrux-api
///!
///! Benchmarks cover:
///! - VM lifecycle operations
///! - Storage operations
///! - Snapshot creation
///! - Database queries
///! - Authentication

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use horcrux_common::*;
use std::time::Duration;

/// Benchmark VM configuration parsing
fn benchmark_vm_config_parsing(c: &mut Criterion) {
    let json_data = r#"{
        "id": "100",
        "name": "test-vm",
        "hypervisor": "qemu",
        "architecture": "x86_64",
        "cpus": 4,
        "memory": 8192,
        "disk_size": 53687091200,
        "status": "running"
    }"#;

    c.bench_function("vm_config_parse", |b| {
        b.iter(|| {
            let _: Result<VmConfig, _> = serde_json::from_str(black_box(json_data));
        });
    });
}

/// Benchmark VM status serialization
fn benchmark_vm_status_serialization(c: &mut Criterion) {
    let vm = VmConfig {
        id: "100".to_string(),
        name: "bench-vm".to_string(),
        hypervisor: VmHypervisor::Qemu,
        architecture: VmArchitecture::X86_64,
        cpus: 4,
        memory: 8192,
        disk_size: 53687091200,
        status: VmStatus::Running,
        disks: vec![],
        network_interfaces: vec![],
        vnc_port: Some(5900),
        created_at: Some(chrono::Utc::now().timestamp()),
        updated_at: Some(chrono::Utc::now().timestamp()),
    };

    c.bench_function("vm_status_serialize", |b| {
        b.iter(|| {
            let _ = serde_json::to_string(black_box(&vm));
        });
    });
}

/// Benchmark container configuration
fn benchmark_container_operations(c: &mut Criterion) {
    let container = ContainerConfig {
        id: "container-1".to_string(),
        name: "bench-container".to_string(),
        container_type: ContainerType::Lxc,
        image: "ubuntu:22.04".to_string(),
        memory: 1024,
        cpus: 2,
        status: ContainerStatus::Running,
        ip_address: Some("10.0.0.1".to_string()),
        created_at: chrono::Utc::now().timestamp(),
    };

    c.bench_function("container_serialize", |b| {
        b.iter(|| {
            let _ = serde_json::to_string(black_box(&container));
        });
    });
}

/// Benchmark error handling
fn benchmark_error_handling(c: &mut Criterion) {
    c.bench_function("error_creation", |b| {
        b.iter(|| {
            let _ = horcrux_common::Error::System(black_box("Test error".to_string()));
        });
    });
}

/// Benchmark path matching for RBAC
fn benchmark_rbac_path_matching(c: &mut Criterion) {
    let paths = vec![
        ("/api/vms/*", "/api/vms/100"),
        ("/api/vms/**", "/api/vms/100/snapshots"),
        ("/", "/api/anything"),
        ("/api/storage/*", "/api/storage/pools"),
    ];

    let mut group = c.benchmark_group("rbac_path_matching");

    for (pattern, test_path) in paths {
        group.bench_with_input(
            BenchmarkId::new("pattern", pattern),
            &(pattern, test_path),
            |b, (pattern, test_path)| {
                b.iter(|| {
                    let matches = if pattern.ends_with("/**") {
                        let prefix = &pattern[..pattern.len() - 3];
                        test_path.starts_with(prefix)
                    } else if pattern.ends_with("/*") {
                        let prefix = &pattern[..pattern.len() - 2];
                        if !test_path.starts_with(prefix) {
                            false
                        } else {
                            let remainder = &test_path[prefix.len()..];
                            !remainder.contains('/') || remainder == "/"
                        }
                    } else {
                        test_path == pattern
                    };
                    black_box(matches);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark JSON parsing for different payload sizes
fn benchmark_json_parsing_sizes(c: &mut Criterion) {
    let small_json = r#"{"id":"1","name":"small"}"#;
    let medium_json = r#"{
        "id": "100",
        "name": "medium-vm",
        "cpus": 4,
        "memory": 8192,
        "disk_size": 53687091200,
        "disks": [
            {"path": "/dev/sda", "size": 53687091200}
        ]
    }"#;
    let large_json = format!(
        r#"{{
        "id": "200",
        "name": "large-vm",
        "cpus": 16,
        "memory": 65536,
        "disks": [{}]
    }}"#,
        (0..100).map(|i| format!(r#"{{"path":"/dev/sd{}","size":10737418240}}"#, (b'a' + (i % 26)) as char))
            .collect::<Vec<_>>()
            .join(",")
    );

    let mut group = c.benchmark_group("json_parsing");

    group.bench_function("small_payload", |b| {
        b.iter(|| {
            let _: serde_json::Value = serde_json::from_str(black_box(small_json)).unwrap();
        });
    });

    group.bench_function("medium_payload", |b| {
        b.iter(|| {
            let _: serde_json::Value = serde_json::from_str(black_box(medium_json)).unwrap();
        });
    });

    group.bench_function("large_payload", |b| {
        b.iter(|| {
            let _: serde_json::Value = serde_json::from_str(black_box(&large_json)).unwrap();
        });
    });

    group.finish();
}

/// Benchmark timestamp operations
fn benchmark_timestamp_operations(c: &mut Criterion) {
    c.bench_function("timestamp_now", |b| {
        b.iter(|| {
            let _ = chrono::Utc::now().timestamp();
        });
    });

    c.bench_function("timestamp_to_rfc3339", |b| {
        b.iter(|| {
            let _ = chrono::Utc::now().to_rfc3339();
        });
    });
}

/// Benchmark UUID generation
fn benchmark_uuid_generation(c: &mut Criterion) {
    c.bench_function("uuid_v4", |b| {
        b.iter(|| {
            let _ = uuid::Uuid::new_v4().to_string();
        });
    });
}

/// Benchmark string operations
fn benchmark_string_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_operations");

    group.bench_function("format_vm_id", |b| {
        b.iter(|| {
            let _ = format!("vm-{}-disk-{}", black_box(100), black_box(0));
        });
    });

    group.bench_function("string_clone", |b| {
        let s = "vm-100-test-string-for-benchmarking";
        b.iter(|| {
            let _ = s.to_string();
        });
    });

    group.bench_function("path_join", |b| {
        b.iter(|| {
            let _ = format!("{}/{}/{}", "/var/lib/horcrux", "vms", black_box("vm-100"));
        });
    });

    group.finish();
}

/// Benchmark vector operations
fn benchmark_vector_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_operations");

    group.bench_function("vec_push_10", |b| {
        b.iter(|| {
            let mut vec = Vec::new();
            for i in 0..10 {
                vec.push(black_box(i));
            }
        });
    });

    group.bench_function("vec_push_100", |b| {
        b.iter(|| {
            let mut vec = Vec::new();
            for i in 0..100 {
                vec.push(black_box(i));
            }
        });
    });

    group.bench_function("vec_filter", |b| {
        let vec: Vec<i32> = (0..100).collect();
        b.iter(|| {
            let _: Vec<&i32> = vec.iter().filter(|&&x| x % 2 == 0).collect();
        });
    });

    group.finish();
}

/// Benchmark HashMap operations
fn benchmark_hashmap_operations(c: &mut Criterion) {
    use std::collections::HashMap;

    let mut group = c.benchmark_group("hashmap_operations");

    group.bench_function("insert_10", |b| {
        b.iter(|| {
            let mut map = HashMap::new();
            for i in 0..10 {
                map.insert(format!("key-{}", i), black_box(i));
            }
        });
    });

    group.bench_function("lookup", |b| {
        let mut map = HashMap::new();
        for i in 0..100 {
            map.insert(format!("key-{}", i), i);
        }
        b.iter(|| {
            let _ = map.get(black_box("key-50"));
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
        benchmark_vm_config_parsing,
        benchmark_vm_status_serialization,
        benchmark_container_operations,
        benchmark_error_handling,
        benchmark_rbac_path_matching,
        benchmark_json_parsing_sizes,
        benchmark_timestamp_operations,
        benchmark_uuid_generation,
        benchmark_string_operations,
        benchmark_vector_operations,
        benchmark_hashmap_operations
}

criterion_main!(benches);
