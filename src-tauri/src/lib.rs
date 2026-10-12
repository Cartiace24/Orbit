use chrono::Utc;
use serde::Serialize;
use std::io;
use sysinfo::System;
use tauri::Manager;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
use winreg::RegKey;

mod diagnostics;
mod history;
mod intelligence;
mod local_ai;
mod search;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InventoryField<T> {
    pub value: Option<T>,
    pub availability: &'static str,
    pub reason: Option<String>,
}

impl<T> InventoryField<T> {
    fn available(value: T) -> Self {
        Self {
            value: Some(value),
            availability: "available",
            reason: None,
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            value: None,
            availability: "unavailable",
            reason: Some(reason.into()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OperatingSystem {
    pub product_name: InventoryField<String>,
    pub edition: InventoryField<String>,
    pub display_version: InventoryField<String>,
    pub build: InventoryField<String>,
    pub architecture: InventoryField<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Computer {
    pub name: InventoryField<String>,
    pub manufacturer: InventoryField<String>,
    pub model: InventoryField<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Processor {
    pub model: InventoryField<String>,
    pub logical_processors: InventoryField<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Memory {
    pub installed_bytes: InventoryField<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemInventory {
    pub collected_at: String,
    pub status: &'static str,
    pub operating_system: OperatingSystem,
    pub computer: Computer,
    pub processor: Processor,
    pub memory: Memory,
}

fn read_string(root: &RegKey, path: &str, name: &str) -> InventoryField<String> {
    match root.open_subkey_with_flags(path, KEY_READ) {
        Ok(key) => match key.get_value::<String, _>(name) {
            Ok(value) if !value.trim().is_empty() => {
                InventoryField::available(value.trim().to_owned())
            }
            Ok(_) => InventoryField::unavailable("Windows returned an empty value."),
            Err(error) => InventoryField::unavailable(registry_reason(&error)),
        },
        Err(error) => InventoryField::unavailable(registry_reason(&error)),
    }
}

fn registry_reason(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "The value is not reported by Windows.".to_owned(),
        io::ErrorKind::PermissionDenied => "Access to this Windows value was denied.".to_owned(),
        _ => "Windows could not provide this value.".to_owned(),
    }
}

fn normalize_product_name(
    mut product_name: InventoryField<String>,
    build: &InventoryField<String>,
) -> InventoryField<String> {
    // Windows 11 keeps "Windows 10" in ProductName for compatibility; builds from 22000 identify Windows 11.
    let build_number = build
        .value
        .as_deref()
        .and_then(|value| value.split('.').next())
        .and_then(|value| value.parse::<u32>().ok());
    if build_number.is_some_and(|number| number >= 22000) {
        if let Some(name) = product_name.value.as_mut() {
            if name.starts_with("Windows 10") {
                *name = name.replacen("Windows 10", "Windows 11", 1);
            }
        }
    }
    product_name
}

fn registry_inventory() -> (OperatingSystem, Computer, InventoryField<String>) {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let current_version = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build_number = read_string(&hklm, current_version, "CurrentBuildNumber");
    let ubr = hklm
        .open_subkey_with_flags(current_version, KEY_READ)
        .ok()
        .and_then(|key| key.get_value::<u32, _>("UBR").ok());
    let build = match (build_number.value, ubr) {
        (Some(number), Some(revision)) => InventoryField::available(format!("{number}.{revision}")),
        (Some(number), None) => InventoryField::available(number),
        (None, _) => InventoryField::unavailable("Windows did not report a build number."),
    };

    let product_name =
        normalize_product_name(read_string(&hklm, current_version, "ProductName"), &build);
    let os = OperatingSystem {
        product_name,
        edition: read_string(&hklm, current_version, "EditionID"),
        display_version: read_string(&hklm, current_version, "DisplayVersion"),
        build,
        architecture: InventoryField::available(std::env::consts::ARCH.to_owned()),
    };
    let bios = r"HARDWARE\DESCRIPTION\System\BIOS";
    let computer = Computer {
        name: std::env::var("COMPUTERNAME")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(InventoryField::available)
            .unwrap_or_else(|| {
                InventoryField::unavailable("The computer name is unavailable in this session.")
            }),
        manufacturer: read_string(&hklm, bios, "SystemManufacturer"),
        model: read_string(&hklm, bios, "SystemProductName"),
    };
    let processor_name = read_string(
        &hklm,
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    );
    (os, computer, processor_name)
}

fn determine_status(fields_available: &[bool]) -> &'static str {
    if fields_available.iter().all(|available| *available) {
        "complete"
    } else {
        "partial"
    }
}

#[tauri::command]
fn get_system_inventory() -> Result<SystemInventory, String> {
    collect_system_inventory()
}

pub(crate) fn collect_system_inventory() -> Result<SystemInventory, String> {
    let (operating_system, computer, registry_processor) = registry_inventory();

    let mut system = System::new();
    system.refresh_memory();
    let memory = if system.total_memory() > 0 {
        InventoryField::available(system.total_memory())
    } else {
        InventoryField::unavailable("Windows did not report installed physical memory.")
    };
    let processor_model = registry_processor;
    let logical_processors = std::thread::available_parallelism()
        .map(|count| InventoryField::available(count.get()))
        .unwrap_or_else(|_| {
            InventoryField::unavailable("Windows did not report the logical processor count.")
        });

    let fields_available = [
        operating_system.product_name.availability == "available",
        operating_system.edition.availability == "available",
        operating_system.display_version.availability == "available",
        operating_system.build.availability == "available",
        operating_system.architecture.availability == "available",
        computer.name.availability == "available",
        computer.manufacturer.availability == "available",
        computer.model.availability == "available",
        processor_model.availability == "available",
        logical_processors.availability == "available",
        memory.availability == "available",
    ];

    Ok(SystemInventory {
        collected_at: Utc::now().to_rfc3339(),
        status: determine_status(&fields_available),
        operating_system,
        computer,
        processor: Processor {
            model: processor_model,
            logical_processors,
        },
        memory: Memory {
            installed_bytes: memory,
        },
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let db_path = app.path().app_data_dir()?.join("diagnostics.sqlite3");
            history::initialize(&db_path).map_err(std::io::Error::other)?;
            app.manage(history::Database(db_path));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_system_inventory,
            diagnostics::run_diagnostic_test,
            diagnostics::save_diagnostic_session,
            history::get_diagnostic_history,
            history::export_diagnostic_report,
            intelligence::analyze_intelligence,
            local_ai::get_local_ai_status,
            local_ai::set_local_ai_enabled,
            local_ai::explain_with_local_ai,
            search::universal_search
        ])
        .run(tauri::generate_context!())
        .expect("failed to run ORBIT desktop application");
}

#[cfg(test)]
mod tests {
    use super::{determine_status, normalize_product_name, read_string, InventoryField};
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    #[test]
    fn missing_registry_value_is_explicitly_unavailable() {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let field = read_string(
            &hklm,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ORBIT_Milestone1_Test_Missing",
            "MissingValue",
        );
        assert_eq!(field.availability, "unavailable");
        assert_eq!(field.value, None);
        assert_eq!(
            field.reason.as_deref(),
            Some("The value is not reported by Windows.")
        );
    }

    #[test]
    fn collection_is_partial_when_a_value_is_missing() {
        assert_eq!(determine_status(&[true, false, true]), "partial");
        assert_eq!(determine_status(&[true, true, true]), "complete");
    }

    #[test]
    fn windows_11_product_name_uses_the_os_build() {
        let product = InventoryField::available("Windows 10 Home Single Language".to_owned());
        let build = InventoryField::available("26200.9457".to_owned());
        let normalized = normalize_product_name(product, &build);
        assert_eq!(
            normalized.value.as_deref(),
            Some("Windows 11 Home Single Language")
        );
    }
}
