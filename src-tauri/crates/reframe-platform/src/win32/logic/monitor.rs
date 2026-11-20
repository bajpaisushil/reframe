//! Monitor identity: stable ids and friendly names from the display configuration.
//!
//! `GetMonitorInfoW` only gives a GDI device name such as `\\.\DISPLAY1`, which can change
//! between boots and when monitors are plugged in. The display configuration API
//! (`QueryDisplayConfig` + `DisplayConfigGetDeviceInfo`) maps each GDI source name to its target
//! monitor's device interface path (derived from the EDID manufacturer/product code and the
//! connector, stable across reboots) and EDID friendly name.

/// One active display path from the display configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayTarget {
    /// `DISPLAYCONFIG_SOURCE_DEVICE_NAME::viewGdiDeviceName`, e.g. `\\.\DISPLAY1`.
    pub gdi_device_name: String,
    /// `DISPLAYCONFIG_TARGET_DEVICE_NAME::monitorFriendlyDeviceName` (may be empty).
    pub friendly_name: String,
    /// `DISPLAYCONFIG_TARGET_DEVICE_NAME::monitorDevicePath` (may be empty).
    pub device_path: String,
    /// The target is a built-in panel (laptop screen).
    pub internal: bool,
}

/// The id and display name reported for a monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorIdentity {
    pub id: String,
    pub name: String,
}

/// Name used for built-in panels without an EDID name.
pub const BUILT_IN_DISPLAY: &str = "Built-in Display";

/// Pick the id and name for the monitor whose GDI device name is `device_name`: the target's
/// device path and friendly name when the display configuration knows them, otherwise the GDI
/// device name and a generic label ("Display 1").
pub fn monitor_identity(device_name: &str, targets: &[DisplayTarget]) -> MonitorIdentity {
    let device_name = device_name.trim();
    let matching: Vec<&DisplayTarget> = targets
        .iter()
        .filter(|t| t.gdi_device_name.trim().eq_ignore_ascii_case(device_name))
        .collect();
    let id = matching
        .iter()
        .map(|t| t.device_path.trim())
        .find(|path| !path.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if device_name.is_empty() {
                "DISPLAY".to_string()
            } else {
                device_name.to_string()
            }
        });
    let name = matching
        .iter()
        .map(|t| t.friendly_name.trim())
        .find(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            matching
                .iter()
                .any(|t| t.internal)
                .then(|| BUILT_IN_DISPLAY.to_string())
        })
        .unwrap_or_else(|| display_label(device_name));
    MonitorIdentity { id, name }
}

/// Generic label for a GDI device name: `\\.\DISPLAY2` → `Display 2`.
pub fn display_label(device_name: &str) -> String {
    let trimmed = device_name.trim();
    let tail = trimmed.rsplit('\\').next().unwrap_or(trimmed);
    let number = tail
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("DISPLAY"))
        .map(|_| &tail[7..])
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    match number {
        Some(n) => format!("Display {n}"),
        None if !tail.is_empty() => tail.to_string(),
        None => "Display".to_string(),
    }
}

/// Whether a `DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY` value denotes a built-in panel: internal,
/// LVDS, embedded DisplayPort or embedded UDI.
pub fn is_internal_output(output_technology: i32) -> bool {
    matches!(output_technology, i32::MIN | 6 | 11 | 13)
}

/// Scale factor for an effective DPI (96 DPI = 100 %). Unknown DPI counts as 100 %.
pub fn scale_factor_from_dpi(dpi: u32) -> f64 {
    if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 }
}

/// Make ids unique by suffixing repeats (`id`, `id#2`, `id#3`...). Distinct monitors should never
/// share a device path, but a duplicate would make two monitors indistinguishable.
pub fn dedupe_ids(ids: &mut [String]) {
    for i in 1..ids.len() {
        if ids[..i].contains(&ids[i]) {
            let base = ids[i].clone();
            let mut n = 2;
            while ids.contains(&format!("{base}#{n}")) {
                n += 1;
            }
            ids[i] = format!("{base}#{n}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELL_PATH: &str =
        r"\\?\DISPLAY#DEL40F0#5&2a5f6b0e&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

    fn target(gdi: &str, friendly: &str, path: &str, internal: bool) -> DisplayTarget {
        DisplayTarget {
            gdi_device_name: gdi.into(),
            friendly_name: friendly.into(),
            device_path: path.into(),
            internal,
        }
    }

    #[test]
    fn matched_target_supplies_id_and_name() {
        let targets = vec![
            target(
                r"\\.\DISPLAY2",
                "LG ULTRAFINE",
                r"\\?\DISPLAY#GSM5B7F#x",
                false,
            ),
            target(r"\\.\DISPLAY1", "DELL U2720Q", DELL_PATH, false),
        ];
        assert_eq!(
            monitor_identity(r"\\.\DISPLAY1", &targets),
            MonitorIdentity {
                id: DELL_PATH.into(),
                name: "DELL U2720Q".into()
            }
        );
        // Case-insensitive match on the GDI name.
        assert_eq!(
            monitor_identity(r"\\.\display2", &targets).name,
            "LG ULTRAFINE"
        );
    }

    #[test]
    fn missing_names_fall_back_sensibly() {
        let targets = vec![
            target(r"\\.\DISPLAY1", "", r"\\?\DISPLAY#BOE0867#1", true),
            target(r"\\.\DISPLAY3", " ", "", false),
        ];
        let laptop = monitor_identity(r"\\.\DISPLAY1", &targets);
        assert_eq!(laptop.id, r"\\?\DISPLAY#BOE0867#1");
        assert_eq!(laptop.name, BUILT_IN_DISPLAY);
        let anonymous = monitor_identity(r"\\.\DISPLAY3", &targets);
        assert_eq!(anonymous.id, r"\\.\DISPLAY3");
        assert_eq!(anonymous.name, "Display 3");
        // Display configuration unavailable (e.g. remote sessions, compatibility layers).
        assert_eq!(
            monitor_identity(r"\\.\DISPLAY7", &[]),
            MonitorIdentity {
                id: r"\\.\DISPLAY7".into(),
                name: "Display 7".into()
            }
        );
        assert_eq!(monitor_identity("", &[]).id, "DISPLAY");
    }

    #[test]
    fn clone_mode_prefers_targets_with_data() {
        let targets = vec![
            target(r"\\.\DISPLAY1", "", "", false),
            target(r"\\.\DISPLAY1", "Projector", r"\\?\DISPLAY#PRJ#1", false),
        ];
        let identity = monitor_identity(r"\\.\DISPLAY1", &targets);
        assert_eq!(identity.id, r"\\?\DISPLAY#PRJ#1");
        assert_eq!(identity.name, "Projector");
    }

    #[test]
    fn display_labels() {
        assert_eq!(display_label(r"\\.\DISPLAY12"), "Display 12");
        assert_eq!(display_label("display4"), "Display 4");
        assert_eq!(display_label(r"\\.\DISPLAYX"), "DISPLAYX");
        assert_eq!(display_label(r"\\.\VGA"), "VGA");
        assert_eq!(display_label(r"\\.\DISP"), "DISP");
        assert_eq!(display_label("ÄÖÜÄÖÜÄÖ"), "ÄÖÜÄÖÜÄÖ");
        assert_eq!(display_label(""), "Display");
    }

    #[test]
    fn output_technologies() {
        assert!(is_internal_output(i32::MIN));
        assert!(is_internal_output(6));
        assert!(is_internal_output(11));
        assert!(!is_internal_output(5)); // HDMI
        assert!(!is_internal_output(10)); // external DisplayPort
        assert!(!is_internal_output(-1));
    }

    #[test]
    fn scale_factors() {
        assert_eq!(scale_factor_from_dpi(96), 1.0);
        assert_eq!(scale_factor_from_dpi(144), 1.5);
        assert_eq!(scale_factor_from_dpi(192), 2.0);
        assert_eq!(scale_factor_from_dpi(0), 1.0);
    }

    #[test]
    fn duplicate_ids_are_suffixed() {
        // Ids that were already unique ("a#2") keep their value; repeats skip taken suffixes.
        let mut ids = vec![
            "a".to_string(),
            "b".into(),
            "a".into(),
            "a".into(),
            "a#2".into(),
        ];
        dedupe_ids(&mut ids);
        assert_eq!(ids, vec!["a", "b", "a#3", "a#4", "a#2"]);
        let mut unique = vec!["x".to_string(), "y".into()];
        dedupe_ids(&mut unique);
        assert_eq!(unique, vec!["x", "y"]);
    }
}
