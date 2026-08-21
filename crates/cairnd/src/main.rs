//! `cairnd(8)`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use api::{Policy, Router, Status};
use cairnd::catalog::Archives;
use cairnd::config::{Config, SandboxMode};
use cairnd::listener::Listener;
use cairnd::server::{Gate, Metrics, Serving, spawn_workers};
use cairnd::{error, info, log, warn};

const DEFAULT_CONFIG: &str = "/etc/cairn/cairn.conf";

const USAGE: &str = "\
usage: cairnd [-c FILE] [--check] [-V] [-h]

  -c, --config FILE   configuration file (default /etc/cairn/cairn.conf)
      --check         parse the configuration, open the archives, and exit
  -V, --version       print the version and exit
  -h, --help          print this message and exit
";

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let mut config_path: Option<PathBuf> = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--config" => {
                if let Some(p) = args.next() {
                    config_path = Some(PathBuf::from(p));
                } else {
                    eprintln!("cairnd: {arg} needs a path");
                    return 2;
                }
            }
            "--check" => check = true,
            "-V" | "--version" => {
                println!("cairnd {}", cairnd::VERSION);
                return 0;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("cairnd: unknown argument {other:?}\n{USAGE}");
                return 2;
            }
        }
    }

    // An explicit path must exist; the default one may not.
    let config = match &config_path {
        Some(p) => match Config::load(p) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("cairnd: {}: {e}", p.display());
                return 1;
            }
        },
        None if Path::new(DEFAULT_CONFIG).exists() => match Config::load(Path::new(DEFAULT_CONFIG))
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("cairnd: {DEFAULT_CONFIG}: {e}");
                return 1;
            }
        },
        None => Config::default(),
    };
    log::set_level(config.log_level);

    match serve(config, check) {
        Ok(code) => code,
        Err(e) => {
            error!("{e}");
            1
        }
    }
}

fn serve(config: Config, check: bool) -> Result<i32, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let config = Arc::new(config);

    // 1. Archives, while the process can still open files.
    let catalog = archive::Catalog::open_dir(&config.archive_dir, config.archive_limits())?;
    for a in catalog.archives() {
        let s = a.summary();
        info!(
            "opened {} as {} ({} entries)",
            a.path().display(),
            s.uuid,
            s.entry_count
        );
    }
    if catalog.archives().is_empty() {
        warn!("no archives in {}", config.archive_dir.display());
    }
    let archives = Arc::new(Archives::new(catalog));

    if check {
        // Opening every archive and then failing to bind is not "ok".
        Listener::preflight(&config.listen)?;
        println!(
            "cairnd: configuration ok, {} archive(s) in {}, will listen on {}",
            archives.inner().archives().len(),
            config.archive_dir.display(),
            config.listen
        );
        return Ok(0);
    }

    // 2. The listener, while the process can still bind.
    let listener = Arc::new(Listener::bind(&config.listen, config.socket_mode)?);
    info!("listening on {}", config.listen);

    // 3. Workers, while the process can still create threads.
    let metrics = Arc::new(Metrics::default());
    let gate = Arc::new(Gate::new());
    let workers = spawn_workers(&listener, &gate, config.max_connections)?;
    gate.wait_for_workers(config.max_connections);

    let seed = random_seed();

    // 4. Confinement. Nothing has been served yet.
    let report = match sandbox::apply(&config.sandbox_policy()) {
        Ok(r) => r,
        Err(incomplete) => {
            error!("{incomplete}");
            error!("`sandbox require` is set, refusing to serve");
            return Ok(1);
        }
    };
    for layer in &report.layers {
        let detail = layer.detail.as_deref().unwrap_or("");
        info!("sandbox {}: {} {detail}", layer.name, layer.state.name());
    }
    if config.sandbox == SandboxMode::Off {
        warn!("sandbox is off");
    }

    // 5. Serve.
    let status = {
        let archives = Arc::clone(&archives);
        let metrics = Arc::clone(&metrics);
        let config = Arc::clone(&config);
        let report = report.clone();
        Box::new(move || {
            let cache = archives.inner().cache_stats();
            Status {
                version: cairnd::VERSION.to_owned(),
                uptime_seconds: started.elapsed().as_secs(),
                listener: config.listen.to_string(),
                archive_count: archives.inner().archives().len() as u64,
                auth_required: config.auth_token.is_some(),
                sandbox: api::Sandbox {
                    required: report.required,
                    layers: report
                        .layers
                        .iter()
                        .map(|l| api::Layer {
                            name: l.name.to_owned(),
                            state: l.state.name().to_owned(),
                            detail: l.detail.clone(),
                        })
                        .collect(),
                },
                cache: api::Cache {
                    budget_bytes: config.cluster_cache_bytes as u64,
                    bytes: cache.bytes as u64,
                    entries: cache.entries as u64,
                    hits: cache.hits,
                    misses: cache.misses,
                    evictions: cache.evictions,
                },
                connections: api::Connections {
                    max: config.max_connections as u64,
                    active: metrics.active.load(Ordering::Relaxed),
                    served: metrics.served.load(Ordering::Relaxed),
                    rejected: metrics.rejected.load(Ordering::Relaxed),
                },
            }
        })
    };

    let router = Router::new(
        Arc::clone(&archives) as Arc<dyn api::Catalog>,
        config.api_limits(),
        Policy {
            auth_token: config.auth_token.clone(),
            content_security_policy: config.content_security_policy.clone(),
        },
        status,
        seed,
    );

    gate.open(Arc::new(Serving {
        router: Arc::new(router),
        config: Arc::clone(&config),
        metrics: Arc::clone(&metrics),
    }));
    info!("serving with {} workers", config.max_connections);

    for w in workers {
        let _ = w.join();
    }
    Ok(0)
}

/// Seed for random entry selection. Not a secret, but not a constant either.
fn random_seed() -> u64 {
    let mut bytes = [0u8; 8];
    // SAFETY: getrandom fills exactly the buffer it is given its length for.
    let rc = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if usize::try_from(rc).is_ok_and(|n| n == bytes.len()) {
        return u64::from_ne_bytes(bytes);
    }
    warn!("getrandom unavailable, seeding from the clock");
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        // A seed, not a clock reading: the low 64 bits are all it needs.
        .map(|d| u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0))
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
}
