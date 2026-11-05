//! Trial S7 measurements for each hello plugin on each backend: component
//! size, cold compile time, load time from the host's own compiled cache,
//! instantiation time, one render call, and memory per plugin.
//!
//! Build both plugins with `crates/plugin-host/trial/build-plugins.sh`, then
//! run a release build:
//!
//! ```sh
//! cargo run --release -p plugin-host --features pulley --example s7_measure
//! ```
//!
//! Memory runs in a fresh child process per plugin and backend, so one
//! measurement's allocations don't hide in another's numbers.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use plugin_host::process_memory;
use plugin_host::trial::{Backend, TrialHost};

const COMPILE_RUNS: usize = 5;
const LOAD_RUNS: usize = 20;
const INSTANTIATE_RUNS: usize = 100;
const CALL_RUNS: usize = 1000;
const MANY_INSTANCES: usize = 20;

struct Plugin {
    name: &'static str,
    path: PathBuf,
}

fn plugins() -> Vec<Plugin> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../target"));
    vec![
        Plugin {
            name: "Rust",
            path: std::env::var_os("S7_RUST_PLUGIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| target_dir.join("wasm32-wasip2/release/s7_rust_hello.wasm")),
        },
        Plugin {
            name: "TypeScript",
            path: std::env::var_os("S7_TYPESCRIPT_PLUGIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    manifest_dir.join("../../sdk/typescript/s7-hello/dist/s7-hello.wasm")
                }),
        },
    ]
}

fn backends() -> Vec<Backend> {
    vec![
        Backend::Cranelift,
        #[cfg(feature = "pulley")]
        Backend::Pulley,
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let [_, mode, plugin, backend, cache] = args.as_slice()
        && mode == "memory"
    {
        let plugin: usize = plugin.parse().expect("plugin index");
        let backend: usize = backend.parse().expect("backend index");
        measure_memory(&plugins()[plugin], backends()[backend], Path::new(cache));
        return;
    }

    let cache = tempfile::tempdir().expect("cache directory");
    if let Some(load) = load_average() {
        println!("Load average at start: {load:.1}");
        println!();
    }
    println!(
        "| Plugin | Backend | Component | Cold compile, median | Cold compile, fastest | Compile CPU, median | Compiled code | Load from cache | Instantiate | Render call | Event + render |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for plugin in plugins() {
        for backend in backends() {
            measure_times(
                &plugin,
                backend,
                &cache_path(cache.path(), &plugin, backend),
            );
        }
    }

    println!();
    println!(
        "| Plugin | Backend | Load compiled code | 1 instance | {MANY_INSTANCES} instances | Per extra instance | Linear memory per store |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|");
    let executable = std::env::current_exe().expect("current executable");
    for (plugin_index, plugin) in plugins().iter().enumerate() {
        for (backend_index, backend) in backends().into_iter().enumerate() {
            let output = Command::new(&executable)
                .arg("memory")
                .arg(plugin_index.to_string())
                .arg(backend_index.to_string())
                .arg(cache_path(cache.path(), plugin, backend))
                .output()
                .expect("memory run starts");
            assert!(
                output.status.success(),
                "memory run failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
    }

    println!();
    for plugin in plugins() {
        let host = TrialHost::new(Backend::Cranelift).expect("host starts");
        let compiled = host.compile(&read(&plugin)).expect("plugin compiles");
        println!("{} plugin imports: {:?}", plugin.name, compiled.imports());
    }
}

fn cache_path(cache: &Path, plugin: &Plugin, backend: Backend) -> PathBuf {
    cache.join(format!("{}-{backend:?}.cwasm", plugin.name))
}

fn read(plugin: &Plugin) -> Vec<u8> {
    std::fs::read(&plugin.path).unwrap_or_else(|error| {
        panic!(
            "{} isn't built ({error}); run crates/plugin-host/trial/build-plugins.sh",
            plugin.path.display()
        )
    })
}

fn measure_times(plugin: &Plugin, backend: Backend, cache_path: &Path) {
    let bytes = read(plugin);

    // Without Wasmtime's `parallel-compilation` feature the calling thread
    // does all the compiling, so its CPU time shows the work even when other
    // processes share the machine.
    let mut compile_wall = Vec::new();
    let mut compile_cpu = Vec::new();
    for _ in 0..COMPILE_RUNS {
        let host = TrialHost::new(backend).expect("host starts");
        let started = Instant::now();
        let cpu_started = thread_cpu_time();
        host.compile(&bytes).expect("plugin compiles");
        compile_wall.push(started.elapsed());
        if let (Some(from), Some(to)) = (cpu_started, thread_cpu_time()) {
            compile_cpu.push(to.saturating_sub(from));
        }
    }
    let fastest_compile = compile_wall.iter().min().copied().expect("compile runs");
    let compile = median(compile_wall.into_iter());
    let compile_cpu = if compile_cpu.is_empty() {
        "n/a".into()
    } else {
        time(median(compile_cpu.into_iter()))
    };

    let host = TrialHost::new(backend).expect("host starts");
    let cached = host
        .compile_to_cache(&bytes, cache_path)
        .expect("plugin compiles into the cache");
    let compiled_size = std::fs::metadata(cache_path).expect("cache file").len();
    let load = median((0..LOAD_RUNS).map(|_| {
        let started = Instant::now();
        host.load_cached(&cached).expect("cached code loads");
        started.elapsed()
    }));

    let compiled = host.load_cached(&cached).expect("cached code loads");
    let instantiate = median((0..INSTANTIATE_RUNS).map(|_| {
        let started = Instant::now();
        let instance = host.instantiate(&compiled).expect("plugin instantiates");
        let elapsed = started.elapsed();
        drop(instance);
        elapsed
    }));

    let mut instance = host.instantiate(&compiled).expect("plugin instantiates");
    let render = median((0..CALL_RUNS).map(|_| {
        let started = Instant::now();
        instance.render().expect("render");
        started.elapsed()
    }));
    let event = median((0..CALL_RUNS).map(|_| {
        let started = Instant::now();
        instance.handle_event("add-one").expect("event");
        instance.render().expect("render");
        started.elapsed()
    }));

    println!(
        "| {} | {backend:?} | {} | {} | {} | {compile_cpu} | {} | {} | {} | {} | {} |",
        plugin.name,
        size(bytes.len() as u64),
        time(compile),
        time(fastest_compile),
        size(compiled_size),
        time(load),
        time(instantiate),
        time(render),
        time(event),
    );
}

fn measure_memory(plugin: &Plugin, backend: Backend, cache_path: &Path) {
    let host = TrialHost::new(backend).expect("host starts");
    // The host loads only code it compiled itself, so this process compiles
    // first and measures from there. Wasmtime maps compiled code and linear
    // memory with mmap, so the compiler's freed heap can't absorb them.
    let cached = host
        .compile_to_cache(&read(plugin), &cache_path.with_extension("memory.cwasm"))
        .expect("plugin compiles into the cache");
    let before_load = process_memory();
    let compiled = host.load_cached(&cached).expect("cached code loads");
    let loaded = process_memory();

    let mut instances = Vec::new();
    let mut add_instance = || {
        let mut instance = host.instantiate(&compiled).expect("plugin instantiates");
        instance.render().expect("render");
        instances.push(instance);
    };
    add_instance();
    let one = process_memory();
    for _ in 1..MANY_INSTANCES {
        add_instance();
    }
    let many = process_memory();
    let linear_memory = instances[0].memory_bytes();

    let growth = |from: Option<u64>, to: Option<u64>| match (from, to) {
        (Some(from), Some(to)) => size(to.saturating_sub(from)),
        _ => "n/a".into(),
    };
    let per_extra = match (one, many) {
        (Some(one), Some(many)) => size(many.saturating_sub(one) / (MANY_INSTANCES as u64 - 1)),
        _ => "n/a".into(),
    };
    println!(
        "| {} | {backend:?} | {} | {} | {} | {} | {} |",
        plugin.name,
        growth(before_load, loaded),
        growth(loaded, one),
        growth(loaded, many),
        per_extra,
        size(linear_memory as u64),
    );
}

fn median(samples: impl Iterator<Item = Duration>) -> Duration {
    let mut samples: Vec<Duration> = samples.collect();
    samples.sort();
    samples[samples.len() / 2]
}

fn time(duration: Duration) -> String {
    let micros = duration.as_secs_f64() * 1e6;
    if micros >= 1000.0 {
        format!("{:.1} ms", micros / 1000.0)
    } else {
        format!("{micros:.0} µs")
    }
}

fn size(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1024.0 * 1024.0 {
        format!("{:.2} MiB", bytes / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", bytes / 1024.0)
    }
}

/// CPU time the calling thread has used.
#[cfg(unix)]
fn thread_cpu_time() -> Option<Duration> {
    // SAFETY: `timespec` is plain data, and `clock_gettime` writes one.
    unsafe {
        let mut time: libc::timespec = std::mem::zeroed();
        let result = libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut time);
        (result == 0).then(|| {
            Duration::new(
                u64::try_from(time.tv_sec).unwrap_or(0),
                u32::try_from(time.tv_nsec).unwrap_or(0),
            )
        })
    }
}

#[cfg(not(unix))]
fn thread_cpu_time() -> Option<Duration> {
    None
}

/// The one-minute load average, which shows how busy the machine was.
#[cfg(unix)]
fn load_average() -> Option<f64> {
    let mut load = [0.0f64; 1];
    // SAFETY: `getloadavg` writes at most one sample into the buffer.
    let samples = unsafe { libc::getloadavg(load.as_mut_ptr(), 1) };
    (samples == 1).then_some(load[0])
}

#[cfg(not(unix))]
fn load_average() -> Option<f64> {
    None
}
