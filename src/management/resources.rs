use serde::Serialize;
use std::{fs, path::Path};

#[derive(Debug, Serialize)]
pub(super) struct Memory {
    total_bytes: u64,
    used_bytes: u64,
}
impl Memory {
    pub(super) fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
    pub(super) fn used_bytes(&self) -> u64 {
        self.used_bytes
    }
    fn new(total_bytes: u64, used_bytes: u64) -> Option<Self> {
        (total_bytes > 0 && used_bytes <= total_bytes).then_some(Self {
            total_bytes,
            used_bytes,
        })
    }
}
#[derive(Serialize)]
pub(super) struct Gpu {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) utilization_percent: Option<u8>,
    pub(super) vram: Option<Memory>,
}
#[derive(Serialize)]
pub(super) struct Resources {
    pub(super) memory: Option<Memory>,
    pub(super) gpus: Option<Vec<Gpu>>,
}
pub(super) fn collect() -> Resources {
    collect_from(Path::new("/proc/meminfo"), Path::new("/sys/class/drm"))
}
fn collect_from(meminfo: &Path, drm: &Path) -> Resources {
    Resources {
        memory: fs::read_to_string(meminfo)
            .ok()
            .and_then(|s| parse_memory(&s)),
        gpus: read_gpus(drm),
    }
}
fn parse_memory(input: &str) -> Option<Memory> {
    let kib = |key: &str| -> Option<u64> {
        let mut words = input
            .lines()
            .find_map(|line| line.strip_prefix(key))?
            .split_whitespace();
        let value = words.next()?.parse::<u64>().ok()?;
        (words.next()? == "kB").then_some(())?;
        value.checked_mul(1024)
    };
    let total = kib("MemTotal:")?;
    let available = kib("MemAvailable:")?;
    Memory::new(total, total.checked_sub(available)?)
}
fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}
fn read_gpus(root: &Path) -> Option<Vec<Gpu>> {
    let mut cards: Vec<_> = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|s| s.strip_prefix("card"))
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    cards.sort_by_key(|entry| entry.file_name());
    Some(
        cards
            .into_iter()
            .map(|card| {
                let device = card.path().join("device");
                let number = |name: &str| read_text(&device.join(name))?.parse::<u64>().ok();
                let vendor = read_text(&device.join("vendor")).unwrap_or_default();
                let vendor = match vendor.as_str() {
                    "0x1002" => "AMD",
                    "0x8086" => "Intel",
                    "0x10de" => "NVIDIA",
                    _ => "GPU",
                };
                let name = read_text(&device.join("product_name")).unwrap_or_else(|| {
                    format!(
                        "{} {}",
                        vendor,
                        read_text(&device.join("device")).unwrap_or_default()
                    )
                    .trim()
                    .to_owned()
                });
                // AMDGPU exports bytes and utilization through the kernel's sysfs interface.
                // https://docs.kernel.org/gpu/amdgpu/driver-misc.html
                let vram = number("mem_info_vram_total")
                    .zip(number("mem_info_vram_used"))
                    .and_then(|(total, used)| Memory::new(total, used));
                Gpu {
                    id: card.file_name().to_string_lossy().into_owned(),
                    name,
                    utilization_percent: number("gpu_busy_percent")
                        .filter(|n| *n <= 100)
                        .map(|n| n as u8),
                    vram,
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_uses_available_including_reclaimable_cache() {
        let memory =
            parse_memory("MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 400 kB\n").unwrap();
        assert_eq!(memory.total_bytes, 1024000);
        assert_eq!(memory.used_bytes, 614400);
        for invalid in [
            "MemTotal: 100 kB\n",
            "MemTotal: 100 kB\nMemAvailable: 101 kB",
            "MemTotal: 100 B\nMemAvailable: 10 kB",
            "MemTotal: 18446744073709551615 kB\nMemAvailable: 1 kB",
        ] {
            assert!(parse_memory(invalid).is_none());
        }
    }
    #[test]
    fn gpu_metrics_are_optional_and_connectors_are_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let device = dir.path().join("card1/device");
        fs::create_dir_all(&device).unwrap();
        fs::create_dir_all(dir.path().join("card1-DP-1")).unwrap();
        fs::create_dir_all(dir.path().join("renderD128")).unwrap();
        for (name, value) in [
            ("vendor", "0x1002"),
            ("device", "0x747e"),
            ("gpu_busy_percent", "42"),
            ("mem_info_vram_total", "1000"),
            ("mem_info_vram_used", "250"),
        ] {
            fs::write(device.join(name), value).unwrap();
        }
        let gpus = read_gpus(dir.path()).unwrap();
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].utilization_percent, Some(42));
        assert_eq!(gpus[0].vram.as_ref().unwrap().used_bytes, 250);
        fs::write(device.join("gpu_busy_percent"), "101").unwrap();
        fs::write(device.join("mem_info_vram_used"), "1001").unwrap();
        let gpus = read_gpus(dir.path()).unwrap();
        assert!(gpus[0].utilization_percent.is_none());
        assert!(gpus[0].vram.is_none());
        assert!(read_gpus(&dir.path().join("missing")).is_none());
    }
}
