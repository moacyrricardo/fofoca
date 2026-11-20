//! Local model servers (whisper.cpp `whisper-server`, llama.cpp `llama-server`) as child processes.
//!
//! fo-foca never links the inference engines: it starts the servers with the flags that won the
//! benchmarks, talks HTTP to them on 127.0.0.1, and stops them to free VRAM when swapping.

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::{data_dir, Config, Device};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Whisper,
    Llm,
}

pub struct ModelServer {
    pub kind: Kind,
    pub base_url: String,
    child: Child,
}

impl ModelServer {
    /// Start a server and wait until it answers. Fails fast if the process dies while loading
    /// (typically: out of GPU memory).
    /// `device` picks GPU (the configured one) or CPU (spec 015).
    pub fn start(kind: Kind, cfg: &Config, device: Device) -> Result<ModelServer> {
        let port = free_port()?;
        let log_dir = data_dir().join("logs");
        std::fs::create_dir_all(&log_dir)?;
        let log_path = log_dir.join(match kind {
            Kind::Whisper => "whisper-server.log",
            Kind::Llm => "llama-server.log",
        });
        let log = File::create(&log_path)?;

        let mut cmd = match kind {
            Kind::Whisper => {
                let w = &cfg.whisper;
                let mut c = Command::new(&w.server);
                c.arg("-m").arg(&w.model);
                match device {
                    Device::Gpu => c.args(["-dev", &w.device.to_string()]),
                    Device::Cpu => c.arg("-ng"),
                };
                c.args(["-t", &w.threads.to_string()]);
                if let Some(vad) = w.vad_model.as_ref().filter(|p| p.exists()) {
                    c.arg("--vad").arg("-vm").arg(vad);
                }
                c
            }
            Kind::Llm => {
                let l = &cfg.llm;
                let mut c = Command::new(&l.server);
                c.arg("-m").arg(&l.model);
                match device {
                    Device::Gpu => c.args(["-dev", &l.device, "-ngl", "99"]),
                    Device::Cpu => c.args(["-dev", "none", "-ngl", "0"]),
                };
                c.args(["-t", &l.threads.to_string()]);
                c.args(["-c", &l.context.to_string(), "-ctk", "q8_0", "-ctv", "q8_0", "-fa", "on"]);
                c.args(["--jinja", "--reasoning", "off"]);
                c
            }
        };
        cmd.args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        crate::proc::die_with_parent(&mut cmd);
        let child = cmd.spawn().with_context(|| {
            let bin = match kind {
                Kind::Whisper => &cfg.whisper.server,
                Kind::Llm => &cfg.llm.server,
            };
            format!("starting {} — check the path in {}", bin.display(), crate::config::config_path().display())
        })?;

        let mut server = ModelServer { kind, base_url: format!("http://127.0.0.1:{port}"), child };
        server
            .wait_ready(Duration::from_secs(180))
            .with_context(|| format!("{kind:?} server did not start — see {}", log_path.display()))?;
        Ok(server)
    }

    fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(2)).build()?;
        let deadline = Instant::now() + timeout;
        let probe = match self.kind {
            Kind::Whisper => format!("{}/", self.base_url),
            Kind::Llm => format!("{}/health", self.base_url),
        };
        loop {
            if let Some(status) = self.child.try_wait()? {
                bail!("process exited while loading ({status})");
            }
            if let Ok(r) = http.get(&probe).send() {
                // llama-server answers 503 while the model is still loading
                if r.status().is_success() {
                    return Ok(());
                }
            }
            if Instant::now() > deadline {
                bail!("timed out after {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stop the server and wait for the process to exit, so its VRAM is released.
    pub fn stop(mut self) {
        self.kill();
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ModelServer {
    fn drop(&mut self) {
        self.kill();
    }
}

fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}
