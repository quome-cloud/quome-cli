//! What this machine says about itself: memory and accelerator. The parsers are pure
//! and run on every OS; only `collect` chooses which commands to ask.
use crate::cloud::clients::runner::{CommandRunner, RunResult};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemFacts {
    pub memory_total_bytes: u64,
    pub memory_free_bytes: u64,
    pub accelerator: String,
}

fn field(out: &str, label: &str) -> Option<u64> {
    let line = out.lines().find(|l| l.trim_start().starts_with(label))?;
    let digits: String = line
        .split(':')
        .nth(1)?
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Free bytes = (free + inactive + speculative) pages × page size.
pub fn parse_vm_stat(out: &str) -> Option<u64> {
    let first = out.lines().next()?;
    let after = first.split("page size of").nth(1)?;
    let page: u64 = after
        .split_whitespace()
        .next()?
        .parse()
        .ok()
        .filter(|p| *p > 0)?;
    let pages = field(out, "Pages free")?
        + field(out, "Pages inactive")?
        + field(out, "Pages speculative").unwrap_or(0);
    Some(pages * page)
}

/// "Apple M3 Ultra" → "apple-m3-ultra"; anything else → "cpu".
pub fn accelerator_from_brand(brand: &str) -> String {
    let b = brand.trim();
    if b.starts_with("Apple") {
        slug(b)
    } else {
        "cpu".into()
    }
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// (MemTotal, MemAvailable) in bytes.
pub fn parse_meminfo(s: &str) -> Option<(u64, u64)> {
    Some((
        field(s, "MemTotal")? * 1024,
        field(s, "MemAvailable")? * 1024,
    ))
}

/// `name,memory.total,memory.free` rows (MiB): (accelerator, total, free) summed.
pub fn parse_nvidia_smi(csv: &str) -> Option<(String, u64, u64)> {
    let (mut name, mut n, mut total, mut free) = (String::new(), 0u64, 0u64, 0u64);
    for line in csv.lines().filter(|l| !l.trim().is_empty()) {
        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        if parts.len() < 3 {
            return None;
        }
        if n == 0 {
            name = parts[0].to_string();
        }
        total += parts[1].parse::<u64>().ok()?;
        free += parts[2].parse::<u64>().ok()?;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let mut acc = slug(&name);
    if !acc.starts_with("nvidia") {
        acc = format!("nvidia-{acc}");
    }
    if n > 1 {
        acc.push_str(&format!("-x{n}"));
    }
    Some((acc, total * 1_048_576, free * 1_048_576))
}

fn stdout(run: &dyn CommandRunner, program: &str, args: &[&str]) -> Option<String> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    match run.run(program, &args) {
        RunResult::Ran {
            ok: true, stdout, ..
        } => Some(stdout),
        _ => None,
    }
}

/// Never fails: anything it cannot read stays 0 / "cpu".
pub fn collect(run: &dyn CommandRunner) -> SystemFacts {
    let mut f = SystemFacts {
        accelerator: "cpu".into(),
        ..Default::default()
    };
    if cfg!(target_os = "macos") {
        if let Some(n) =
            stdout(run, "sysctl", &["-n", "hw.memsize"]).and_then(|s| s.trim().parse::<u64>().ok())
        {
            f.memory_total_bytes = n;
        }
        if let Some(n) = stdout(run, "vm_stat", &[]).and_then(|s| parse_vm_stat(&s)) {
            f.memory_free_bytes = n;
        }
        if let Some(b) = stdout(run, "sysctl", &["-n", "machdep.cpu.brand_string"]) {
            f.accelerator = accelerator_from_brand(&b);
        }
    } else {
        if let Some((t, a)) = stdout(run, "cat", &["/proc/meminfo"]).and_then(|s| parse_meminfo(&s))
        {
            f.memory_total_bytes = t;
            f.memory_free_bytes = a;
        }
        let q = [
            "--query-gpu=name,memory.total,memory.free",
            "--format=csv,noheader,nounits",
        ];
        if let Some((acc, t, fr)) = stdout(run, "nvidia-smi", &q).and_then(|s| parse_nvidia_smi(&s))
        {
            f.accelerator = acc;
            f.memory_total_bytes = t;
            f.memory_free_bytes = fr;
        }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::clients::runner::fake::Fake;

    #[test]
    fn vm_stat_free_bytes() {
        let s = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
Pages free:                               100.\n\
Pages active:                          500000.\n\
Pages inactive:                            50.\n\
Pages speculative:                         10.\n\
Pages wired down:                       90000.\n";
        assert_eq!(parse_vm_stat(s), Some(160 * 16384));
        assert_eq!(parse_vm_stat("garbage"), None);
    }

    #[test]
    fn brand_to_accelerator() {
        assert_eq!(accelerator_from_brand("Apple M3 Ultra"), "apple-m3-ultra");
        assert_eq!(accelerator_from_brand("Apple M4 Max"), "apple-m4-max");
        assert_eq!(
            accelerator_from_brand("Intel(R) Core(TM) i9-9980HK CPU @ 2.40GHz"),
            "cpu"
        );
    }

    #[test]
    fn meminfo() {
        let s = "MemTotal:       65843212 kB\nMemFree:  1 kB\nMemAvailable:   51234567 kB\n";
        assert_eq!(parse_meminfo(s), Some((65843212 * 1024, 51234567 * 1024)));
    }

    #[test]
    fn nvidia_two_gpus() {
        let csv = "NVIDIA GeForce RTX 4090, 24564, 23000\nNVIDIA GeForce RTX 4090, 24564, 24000";
        assert_eq!(
            parse_nvidia_smi(csv),
            Some((
                "nvidia-geforce-rtx-4090-x2".into(),
                49128 * 1_048_576,
                47000 * 1_048_576
            ))
        );
        assert_eq!(
            parse_nvidia_smi("NVIDIA A100, 40960, 40000").unwrap().0,
            "nvidia-a100"
        );
        assert_eq!(parse_nvidia_smi(""), None);
    }

    #[test]
    fn collect_survives_missing_tools() {
        let f = Fake::new(|_, _| RunResult::NotFound);
        assert_eq!(
            collect(&f),
            SystemFacts {
                memory_total_bytes: 0,
                memory_free_bytes: 0,
                accelerator: "cpu".into()
            }
        );
    }
}
