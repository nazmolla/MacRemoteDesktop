//! Reply grammar of the `macrdpdisplay` helper (one line per command).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostMode {
    pub display_id: u32,
    pub points_w: u32,
    pub points_h: u32,
    pub pixels_w: u32,
    pub pixels_h: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostReply {
    Ok(HostMode),
    /// The host selected a mode explicitly and can no longer re-mode; start a fresh host.
    NeedsReplace,
    Err(String),
}

pub fn parse_reply(line: &str) -> HostReply {
    let line = line.trim();
    if line == "err needs-replace" {
        return HostReply::NeedsReplace;
    }
    if let Some(msg) = line.strip_prefix("err ") {
        return HostReply::Err(msg.to_string());
    }
    let malformed = || HostReply::Err(format!("malformed reply: {line}"));
    let Some(rest) = line.strip_prefix("ok ") else {
        return malformed();
    };
    let nums: Vec<u32> = match rest.split(' ').map(str::parse).collect() {
        Ok(v) => v,
        Err(_) => return malformed(),
    };
    match nums[..] {
        [display_id, points_w, points_h, pixels_w, pixels_h] if display_id != 0 => {
            HostReply::Ok(HostMode {
                display_id,
                points_w,
                points_h,
                pixels_w,
                pixels_h,
            })
        }
        _ => malformed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_parses_all_fields() {
        let line = "ok 139 1280 720 2560 1440";
        let result = parse_reply(line);
        match result {
            HostReply::Ok(HostMode {
                display_id,
                points_w,
                points_h,
                pixels_w,
                pixels_h,
            }) => {
                assert_eq!(display_id, 139);
                assert_eq!(points_w, 1280);
                assert_eq!(points_h, 720);
                assert_eq!(pixels_w, 2560);
                assert_eq!(pixels_h, 1440);
            }
            other => panic!("Expected Ok, got {:?}", other),
        }
    }

    #[test]
    fn ok_with_wrong_field_count_is_malformed() {
        let line = "ok 139 1280 720";
        let result = parse_reply(line);
        match result {
            HostReply::Err(msg) => assert!(msg.contains("malformed reply: ok 139 1280 720")),
            other => panic!("Expected Err, got {:?}", other),
        }
    }

    #[test]
    fn ok_with_zero_id_is_malformed() {
        let line = "ok 0 1280 720 2560 1440";
        let result = parse_reply(line);
        match result {
            HostReply::Err(msg) => {
                assert!(msg.contains("malformed reply: ok 0 1280 720 2560 1440"))
            }
            other => panic!("Expected Err, got {:?}", other),
        }
    }

    #[test]
    fn ok_with_non_numeric_is_malformed() {
        let line = "ok 1 a 2 3 4";
        let result = parse_reply(line);
        match result {
            HostReply::Err(msg) => assert!(msg.contains("malformed reply: ok 1 a 2 3 4")),
            other => panic!("Expected Err, got {:?}", other),
        }
    }

    #[test]
    fn needs_replace() {
        let line = "err needs-replace";
        let result = parse_reply(line);
        match result {
            HostReply::NeedsReplace => {}
            other => panic!("Expected NeedsReplace, got {:?}", other),
        }
    }

    #[test]
    fn err_keeps_message() {
        let line = "err descriptor rejected";
        let result = parse_reply(line);
        match result {
            HostReply::Err(msg) => assert_eq!(msg, "descriptor rejected"),
            other => panic!("Expected Err, got {:?}", other),
        }
    }

    #[test]
    fn empty_is_malformed() {
        let line = "";
        let result = parse_reply(line);
        match result {
            HostReply::Err(msg) => assert!(msg.contains("malformed reply: ")),
            other => panic!("Expected Err, got {:?}", other),
        }
    }

    #[test]
    fn trailing_whitespace_is_trimmed() {
        let line = "ok 1 2 3 4 5\n";
        let result = parse_reply(line);
        match result {
            HostReply::Ok(HostMode {
                display_id,
                points_w,
                points_h,
                pixels_w,
                pixels_h,
            }) => {
                assert_eq!(display_id, 1);
                assert_eq!(points_w, 2);
                assert_eq!(points_h, 3);
                assert_eq!(pixels_w, 4);
                assert_eq!(pixels_h, 5);
            }
            other => panic!("Expected Ok, got {:?}", other),
        }
    }
}
