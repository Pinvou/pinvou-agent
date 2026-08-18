use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ThermalSource {
    Platform,
    Cpu,
}

/// Return the hottest trusted CPU/package temperature from Linux thermal sysfs.
/// Unknown zones (battery, radio, anonymous firmware sensors) are ignored. When
/// no CPU-specific zone exists, a known ACPI/platform thermal source is used as
/// a conservative fallback.
pub fn temperature_c() -> Option<f64> {
    temperature_c_from_root(Path::new("/sys/class/thermal"))
}

fn temperature_c_from_root(root: &Path) -> Option<f64> {
    let mut cpu_temperatures = Vec::new();
    let mut platform_temperatures = Vec::new();

    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let file_name = entry.file_name();
        if !file_name.to_string_lossy().starts_with("thermal_zone") {
            continue;
        }

        let source = std::fs::read_to_string(entry.path().join("type"))
            .ok()
            .and_then(|value| classify_source(&value));
        let value = std::fs::read_to_string(entry.path().join("temp"))
            .ok()
            .and_then(|value| parse_millidegrees_c(&value));

        match (source, value) {
            (Some(ThermalSource::Cpu), Some(value)) => cpu_temperatures.push(value),
            (Some(ThermalSource::Platform), Some(value)) => platform_temperatures.push(value),
            _ => {}
        }
    }

    hottest(&cpu_temperatures).or_else(|| hottest(&platform_temperatures))
}

fn classify_source(kind: &str) -> Option<ThermalSource> {
    let normalized = kind.trim().to_ascii_lowercase().replace(['-', '_'], " ");
    let compact = normalized.replace(' ', "");

    let is_cpu = normalized.contains("cpu")
        || normalized.contains("package")
        || normalized.contains("coretemp")
        || normalized.contains("tctl")
        || normalized.contains("tdie")
        || compact.contains("x86pkgtemp")
        || compact == "soc";
    if is_cpu {
        return Some(ThermalSource::Cpu);
    }

    matches!(compact.as_str(), "acpitz" | "int3400thermal").then_some(ThermalSource::Platform)
}

fn parse_millidegrees_c(text: &str) -> Option<f64> {
    let millidegrees = text.trim().parse::<i64>().ok()?;
    let value = millidegrees as f64 / 1_000.0;
    // Linux thermal-zone temperatures are millidegrees Celsius. Reject values
    // outside a physically credible electronics range instead of feeding a bad
    // firmware sentinel into the deterministic resource governor.
    (-20.0..=125.0).contains(&value).then_some(value)
}

fn hottest(values: &[f64]) -> Option<f64> {
    values.iter().copied().max_by(f64::total_cmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_megabook_cpu_zone_types_and_known_platform_fallbacks() {
        for kind in ["TCPU", "TCPU_PCI", "x86_pkg_temp", "coretemp", "Tctl"] {
            assert_eq!(classify_source(kind), Some(ThermalSource::Cpu));
        }
        assert_eq!(classify_source("acpitz"), Some(ThermalSource::Platform));
        assert_eq!(
            classify_source("INT3400 Thermal"),
            Some(ThermalSource::Platform)
        );
    }

    #[test]
    fn excludes_untrusted_or_unrelated_thermal_zones() {
        for kind in ["iwlwifi_1", "SEN3", "BAT0", "nvme"] {
            assert_eq!(classify_source(kind), None);
        }
    }

    #[test]
    fn parses_millidegrees_and_rejects_bad_sensor_values() {
        assert_eq!(parse_millidegrees_c("43000\n"), Some(43.0));
        assert_eq!(parse_millidegrees_c("-5000"), Some(-5.0));
        assert_eq!(parse_millidegrees_c("not-a-temperature"), None);
        assert_eq!(parse_millidegrees_c("126000"), None);
        assert_eq!(parse_millidegrees_c("-21000"), None);
    }

    #[test]
    fn cpu_temperature_wins_over_hotter_platform_fallback() {
        let cpu = [41.0, 43.0];
        let platform = [70.0];
        assert_eq!(hottest(&cpu).or_else(|| hottest(&platform)), Some(43.0));
        assert_eq!(hottest(&[]).or_else(|| hottest(&platform)), Some(70.0));
    }
}
