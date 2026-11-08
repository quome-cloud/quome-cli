//! A tiny injected process runner shared by the adapters that register through a
//! client's own CLI. Tests swap in a fake so nothing touches the real machine.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunResult {
    /// The program is not on PATH.
    NotFound,
    Ran {
        ok: bool,
        stdout: String,
        stderr: String,
    },
}

pub trait CommandRunner {
    fn run(&self, program: &str, args: &[String]) -> RunResult;
}

pub struct RealRunner;

impl CommandRunner for RealRunner {
    fn run(&self, program: &str, args: &[String]) -> RunResult {
        let out = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output();
        match out {
            Ok(o) => RunResult::Ran {
                ok: o.status.success(),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            },
            Err(_) => RunResult::NotFound,
        }
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    type Script = Box<dyn Fn(&str, &[String]) -> RunResult>;

    /// Records every call; answers with `script`.
    pub struct Fake {
        pub calls: Mutex<Vec<String>>,
        script: Script,
    }

    impl Fake {
        pub fn new(script: impl Fn(&str, &[String]) -> RunResult + 'static) -> Self {
            Fake {
                calls: Mutex::new(Vec::new()),
                script: Box::new(script),
            }
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, program: &str, args: &[String]) -> RunResult {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            (self.script)(program, args)
        }
    }

    pub fn ok(stdout: &str) -> RunResult {
        RunResult::Ran {
            ok: true,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn fail(stderr: &str) -> RunResult {
        RunResult::Ran {
            ok: false,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}
