//! The MIDI controller mapping table in the settings: one row per binding
//! with readable names, and the command that removes it.

use fourfloor_model::command::UnmapController;
use fourfloor_model::{Command, Project};

/// One controller binding as the table shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct MappingRow {
    /// `"any"` or `"1"`..`"16"`.
    pub channel: String,
    /// Controller number.
    pub controller: u8,
    /// Track holding the device, or `"?"` if it is gone.
    pub track: String,
    /// Device name.
    pub device: String,
    /// Parameter name.
    pub param: String,
    /// Parameter value at controller 0 and 127.
    pub range: (f32, f32),
    /// The command that removes the binding.
    pub remove: Command,
}

/// Every binding in `project`, in the order they were made.
pub fn mapping_rows(project: &Project) -> Vec<MappingRow> {
    project
        .controller_map
        .iter()
        .map(|mapping| {
            let found = project.tracks.iter().find_map(|track| {
                track
                    .devices
                    .iter()
                    .find(|d| d.id == mapping.device)
                    .map(|device| (track, device))
            });
            let (track, device, param) = match found {
                Some((track, device)) => (
                    track.name.clone(),
                    device.name.clone(),
                    device
                        .params
                        .iter()
                        .find(|p| p.id == mapping.param)
                        .map_or_else(|| format!("#{}", mapping.param.0), |p| p.name.clone()),
                ),
                None => (
                    "?".to_owned(),
                    "?".to_owned(),
                    format!("#{}", mapping.param.0),
                ),
            };
            MappingRow {
                channel: mapping
                    .channel
                    .map_or_else(|| "any".to_owned(), |c| (u16::from(c) + 1).to_string()),
                controller: mapping.controller,
                track,
                device,
                param,
                range: (mapping.min, mapping.max),
                remove: Command::UnmapController(UnmapController {
                    channel: mapping.channel,
                    controller: mapping.controller,
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_model::command::MapController;
    use fourfloor_model::CommandBus;

    #[test]
    fn rows_name_the_parameter_and_remove_the_binding() {
        let registry = crate::services::build_registry(false).expect("registry");
        let project = crate::project::build_demo_project(&registry).expect("demo");
        let (device, param) = project
            .tracks
            .iter()
            .flat_map(|t| t.devices.iter())
            .find_map(|d| d.params.first().map(|p| (d.id, p.id)))
            .expect("a device with a parameter");
        let mut bus = CommandBus::new(project);
        bus.execute(MapController {
            channel: Some(0),
            controller: 74,
            device,
            param,
            min: None,
            max: None,
        })
        .expect("maps");
        let rows = mapping_rows(bus.project());
        let row = rows
            .iter()
            .find(|r| r.controller == 74)
            .expect("the new row");
        assert_eq!(row.channel, "1");
        assert_ne!(row.device, "?");
        assert!(!row.param.starts_with('#'), "named: {}", row.param);
        let before = bus.project().controller_map.len();
        bus.execute(row.remove.clone()).expect("unmaps");
        assert_eq!(bus.project().controller_map.len(), before - 1);
    }
}
