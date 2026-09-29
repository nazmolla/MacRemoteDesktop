//! Classify the Mac's online displays (spec §7.5).

/// Vendor id macrdp gives its own virtual displays ("macr").
pub const OUR_VENDOR: u32 = 0x6D61_6372;
/// Vendor id of the virtual display Apple Screen Sharing creates ("ARD screen"), observed on macOS 27.
pub const SCREEN_SHARING_VENDOR: u32 = 0x896;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnlineDisplay {
    pub id: u32,
    pub builtin: bool,
    pub vendor: u32,
}

/// Displays a person could be looking at in the room: built-in panels and real
/// monitors, excluding our own and Screen Sharing's virtual displays.
pub fn physical_displays(online: &[OnlineDisplay]) -> Vec<u32> {
    online
        .iter()
        .filter(|d| d.builtin || (d.vendor != OUR_VENDOR && d.vendor != SCREEN_SHARING_VENDOR))
        .map(|d| d.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_sharing_and_our_displays_are_not_physical() {
        let online = [
            OnlineDisplay {
                id: 5,
                builtin: false,
                vendor: SCREEN_SHARING_VENDOR,
            },
            OnlineDisplay {
                id: 12,
                builtin: false,
                vendor: OUR_VENDOR,
            },
        ];
        assert!(physical_displays(&online).is_empty());
    }

    #[test]
    fn builtin_and_external_monitors_are_physical() {
        let online = [
            OnlineDisplay {
                id: 1,
                builtin: true,
                vendor: 0x610,
            },
            OnlineDisplay {
                id: 2,
                builtin: false,
                vendor: 0x1e6d,
            },
            OnlineDisplay {
                id: 12,
                builtin: false,
                vendor: OUR_VENDOR,
            },
        ];
        assert_eq!(physical_displays(&online), vec![1, 2]);
    }
}
