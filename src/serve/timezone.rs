//! Linux-native timezone resolution and civil time calculations.
//!
//! Provides zero-dependency timezone offset resolution by either parsing explicit ISO
//! offset strings (e.g. `+08:00`, `UTC+8`, `-05:00`) or reading Linux system zoneinfo files
//! directly from `/usr/share/zoneinfo` and `/etc/localtime` (RFC 8536 / TZif format).

use std::path::{Path, PathBuf};

/// Resolves a timezone string into UTC offset seconds and a human-readable label.
///
/// # Arguments
/// * `tz_str` - Timezone string (e.g., `"+08:00"`, `"UTC+8"`, `"Asia/Taipei"`, `"local"`, `"UTC"`).
/// * `epoch_secs` - UTC timestamp in seconds since UNIX epoch, used for dynamic DST calculation.
///
/// # Returns
/// `(offset_seconds, label)` — e.g. `(28800, "+08:00")` or `(28800, "CST")`.
pub fn resolve_timezone_offset(tz_str: &str, epoch_secs: u64) -> (i32, String) {
    let trimmed = tz_str.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("utc")
        || trimmed.eq_ignore_ascii_case("z")
    {
        return (0, "UTC".to_string());
    }

    // 1. Fast path: parse fixed numeric offset (e.g. "+08:00", "+8", "UTC+8", "-05:00")
    if let Some(offset) = parse_fixed_offset(trimmed) {
        return (offset, format_offset_label(offset));
    }

    // 2. Linux native zoneinfo resolution (/usr/share/zoneinfo/... or /etc/localtime)
    if let Some((offset, abbr)) = read_linux_zoneinfo(trimmed, epoch_secs) {
        return (offset, abbr);
    }

    tracing::warn!(
        "Invalid or unfound timezone '{}', falling back to UTC",
        tz_str
    );
    (0, "UTC".to_string())
}

/// Parses fixed numeric offset strings (e.g. `"+08:00"`, `"+0800"`, `"+8"`, `"-05:00"`, `"UTC+8"`, `"GMT-5"`).
pub fn parse_fixed_offset(s: &str) -> Option<i32> {
    let s = s.trim();
    let s = s
        .strip_prefix("UTC")
        .or_else(|| s.strip_prefix("utc"))
        .or_else(|| s.strip_prefix("GMT"))
        .or_else(|| s.strip_prefix("gmt"))
        .unwrap_or(s);

    let (sign, rest) = if let Some(stripped) = s.strip_prefix('+') {
        (1, stripped)
    } else if let Some(stripped) = s.strip_prefix('-') {
        (-1, stripped)
    } else {
        return None;
    };

    if let Some((h_str, m_str)) = rest.split_once(':') {
        let h: i32 = h_str.parse().ok()?;
        let m: i32 = m_str.parse().ok()?;
        if h.abs() > 23 || m > 59 {
            return None;
        }
        Some(sign * (h * 3600 + m * 60))
    } else if rest.len() == 4 && rest.chars().all(|c| c.is_ascii_digit()) {
        let h: i32 = rest[0..2].parse().ok()?;
        let m: i32 = rest[2..4].parse().ok()?;
        if h > 23 || m > 59 {
            return None;
        }
        Some(sign * (h * 3600 + m * 60))
    } else if let Ok(h) = rest.parse::<i32>() {
        if h.abs() > 23 {
            return None;
        }
        Some(sign * h * 3600)
    } else {
        None
    }
}

/// Formats UTC offset seconds as `+HH:MM` or `-HH:MM`.
pub fn format_offset_label(offset_secs: i32) -> String {
    let sign = if offset_secs >= 0 { '+' } else { '-' };
    let abs = offset_secs.unsigned_abs() as usize;
    let h = abs / 3600;
    let m = (abs % 3600) / 60;
    format!("{}{:02}:{:02}", sign, h, m)
}

/// Reads Linux native zoneinfo file (RFC 8536 / TZif format) from `/usr/share/zoneinfo` or `/etc/localtime`.
pub fn read_linux_zoneinfo(name_or_path: &str, epoch_secs: u64) -> Option<(i32, String)> {
    let path = if name_or_path.eq_ignore_ascii_case("local")
        || name_or_path.eq_ignore_ascii_case("system")
    {
        PathBuf::from("/etc/localtime")
    } else if name_or_path.starts_with('/') {
        PathBuf::from(name_or_path)
    } else {
        Path::new("/usr/share/zoneinfo").join(name_or_path)
    };

    let data = std::fs::read(&path).ok()?;
    parse_tzif(&data, epoch_secs)
}

/// Parses a TZif (RFC 8536) byte slice and computes offset and abbreviation for `epoch_secs`.
pub fn parse_tzif(data: &[u8], epoch_secs: u64) -> Option<(i32, String)> {
    if data.len() < 44 || &data[0..4] != b"TZif" {
        return None;
    }

    let version = data[4];
    let isgmtcnt_32 = u32::from_be_bytes(data[20..24].try_into().ok()?) as usize;
    let isstdcnt_32 = u32::from_be_bytes(data[24..28].try_into().ok()?) as usize;
    let leapcnt_32 = u32::from_be_bytes(data[28..32].try_into().ok()?) as usize;
    let timecnt_32 = u32::from_be_bytes(data[32..36].try_into().ok()?) as usize;
    let typecnt_32 = u32::from_be_bytes(data[36..40].try_into().ok()?) as usize;
    let charcnt_32 = u32::from_be_bytes(data[40..44].try_into().ok()?) as usize;

    let v1_data_len =
        timecnt_32 * 5 + typecnt_32 * 6 + charcnt_32 + leapcnt_32 * 8 + isstdcnt_32 + isgmtcnt_32;
    let v2_header_offset = 44 + v1_data_len;

    // Check if TZif version 2 or 3 64-bit header is present
    if (version == b'2' || version == b'3')
        && data.len() >= v2_header_offset + 44
        && &data[v2_header_offset..v2_header_offset + 4] == b"TZif"
    {
        let v2_data = &data[v2_header_offset..];
        let leapcnt_64 = u32::from_be_bytes(v2_data[28..32].try_into().ok()?) as usize;
        let timecnt_64 = u32::from_be_bytes(v2_data[32..36].try_into().ok()?) as usize;
        let typecnt_64 = u32::from_be_bytes(v2_data[36..40].try_into().ok()?) as usize;
        let charcnt_64 = u32::from_be_bytes(v2_data[40..44].try_into().ok()?) as usize;

        let mut cursor = 44;
        let req_len = cursor + timecnt_64 * 8 + timecnt_64 + typecnt_64 * 6 + charcnt_64;
        if v2_data.len() >= req_len {
            let trans_times_bytes = &v2_data[cursor..cursor + timecnt_64 * 8];
            cursor += timecnt_64 * 8;
            let trans_types = &v2_data[cursor..cursor + timecnt_64];
            cursor += timecnt_64;
            let ttinfos_bytes = &v2_data[cursor..cursor + typecnt_64 * 6];
            cursor += typecnt_64 * 6;
            let abbr_chars = &v2_data[cursor..cursor + charcnt_64];

            let cur_epoch = epoch_secs as i64;
            let mut type_idx = 0;
            if timecnt_64 > 0 {
                let first_t =
                    i64::from_be_bytes(trans_times_bytes[0..8].try_into().unwrap_or_default());
                if cur_epoch < first_t {
                    // Before first transition: find first non-DST ttinfo
                    for i in 0..typecnt_64 {
                        let is_dst = ttinfos_bytes[i * 6 + 4];
                        if is_dst == 0 {
                            type_idx = i;
                            break;
                        }
                    }
                } else {
                    for i in 0..timecnt_64 {
                        let t = i64::from_be_bytes(
                            trans_times_bytes[i * 8..(i + 1) * 8]
                                .try_into()
                                .unwrap_or_default(),
                        );
                        if cur_epoch >= t {
                            type_idx = trans_types[i] as usize;
                        } else {
                            break;
                        }
                    }
                }
            }

            if type_idx < typecnt_64 {
                let ttinfo = &ttinfos_bytes[type_idx * 6..(type_idx + 1) * 6];
                let offset_secs = i32::from_be_bytes(ttinfo[0..4].try_into().unwrap_or_default());
                let abbr_ind = ttinfo[5] as usize;
                let abbr = if abbr_ind < abbr_chars.len() {
                    let end = abbr_chars[abbr_ind..]
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(abbr_chars.len() - abbr_ind);
                    String::from_utf8_lossy(&abbr_chars[abbr_ind..abbr_ind + end]).to_string()
                } else {
                    format_offset_label(offset_secs)
                };
                return Some((offset_secs, abbr));
            }
        }
    }

    // Fallback: Parse 32-bit block
    let mut cursor = 44;
    let req_len = cursor + timecnt_32 * 4 + timecnt_32 + typecnt_32 * 6 + charcnt_32;
    if data.len() < req_len {
        return None;
    }

    let trans_times_bytes = &data[cursor..cursor + timecnt_32 * 4];
    cursor += timecnt_32 * 4;
    let trans_types = &data[cursor..cursor + timecnt_32];
    cursor += timecnt_32;
    let ttinfos_bytes = &data[cursor..cursor + typecnt_32 * 6];
    cursor += typecnt_32 * 6;
    let abbr_chars = &data[cursor..cursor + charcnt_32];

    let cur_epoch = (epoch_secs as i64).min(i32::MAX as i64) as i32;
    let mut type_idx = 0;
    if timecnt_32 > 0 {
        let first_t = i32::from_be_bytes(trans_times_bytes[0..4].try_into().unwrap_or_default());
        if cur_epoch < first_t {
            for i in 0..typecnt_32 {
                let is_dst = ttinfos_bytes[i * 6 + 4];
                if is_dst == 0 {
                    type_idx = i;
                    break;
                }
            }
        } else {
            for i in 0..timecnt_32 {
                let t = i32::from_be_bytes(
                    trans_times_bytes[i * 4..(i + 1) * 4]
                        .try_into()
                        .unwrap_or_default(),
                );
                if cur_epoch >= t {
                    type_idx = trans_types[i] as usize;
                } else {
                    break;
                }
            }
        }
    }

    if type_idx < typecnt_32 {
        let ttinfo = &ttinfos_bytes[type_idx * 6..(type_idx + 1) * 6];
        let offset_secs = i32::from_be_bytes(ttinfo[0..4].try_into().unwrap_or_default());
        let abbr_ind = ttinfo[5] as usize;
        let abbr = if abbr_ind < abbr_chars.len() {
            let end = abbr_chars[abbr_ind..]
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(abbr_chars.len() - abbr_ind);
            String::from_utf8_lossy(&abbr_chars[abbr_ind..abbr_ind + end]).to_string()
        } else {
            format_offset_label(offset_secs)
        };
        return Some((offset_secs, abbr));
    }

    None
}

/// Howard Hinnant's algorithm to compute civil date and time from seconds since 1970-01-01 00:00:00 UTC.
pub fn civil_from_timestamp(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as i64;
    let secs_of_day = (secs % 86400) as u32;
    let hours = secs_of_day / 3600;
    let mins = (secs_of_day % 3600) / 60;
    let secs = secs_of_day % 60;

    // Howard Hinnant's algorithm (civil date from days since 1970-01-01)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y } as i32;

    (year, m, d, hours, mins, secs)
}

/// Computes civil date and time applying a timezone offset (in seconds) to UTC epoch seconds.
pub fn civil_from_timestamp_with_offset(
    utc_epoch_secs: u64,
    offset_secs: i32,
) -> (i32, u32, u32, u32, u32, u32) {
    let local_epoch_secs = if offset_secs >= 0 {
        utc_epoch_secs.saturating_add(offset_secs as u64)
    } else {
        utc_epoch_secs.saturating_sub((-offset_secs) as u64)
    };
    civil_from_timestamp(local_epoch_secs)
}

/// Formats the date string (`YYYY-MM-DD`) for a given UTC timestamp and timezone offset.
pub fn format_date_with_tz(utc_epoch_secs: u64, offset_secs: i32) -> String {
    let (y, m, d, _, _, _) = civil_from_timestamp_with_offset(utc_epoch_secs, offset_secs);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Formats the datetime string (`YYYY-MM-DD HH:MM:SS {tz_label}`) for a given UTC timestamp and timezone offset.
pub fn format_datetime_with_tz(utc_epoch_secs: u64, offset_secs: i32, tz_label: &str) -> String {
    let (y, m, d, h, min, s) = civil_from_timestamp_with_offset(utc_epoch_secs, offset_secs);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}",
        y, m, d, h, min, s, tz_label
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fixed_offset() {
        assert_eq!(parse_fixed_offset("+08:00"), Some(28800));
        assert_eq!(parse_fixed_offset("+0800"), Some(28800));
        assert_eq!(parse_fixed_offset("+8"), Some(28800));
        assert_eq!(parse_fixed_offset("UTC+8"), Some(28800));
        assert_eq!(parse_fixed_offset("GMT+8"), Some(28800));
        assert_eq!(parse_fixed_offset("-05:00"), Some(-18000));
        assert_eq!(parse_fixed_offset("-0500"), Some(-18000));
        assert_eq!(parse_fixed_offset("-5"), Some(-18000));
        assert_eq!(parse_fixed_offset("UTC-5"), Some(-18000));
        assert_eq!(parse_fixed_offset("+05:30"), Some(19800));
        assert_eq!(parse_fixed_offset("+00:00"), Some(0));
        assert_eq!(parse_fixed_offset("invalid"), None);
    }

    #[test]
    fn test_resolve_timezone_offset() {
        assert_eq!(resolve_timezone_offset("UTC", 0), (0, "UTC".to_string()));
        assert_eq!(resolve_timezone_offset("Z", 0), (0, "UTC".to_string()));
        assert_eq!(resolve_timezone_offset("", 0), (0, "UTC".to_string()));
        assert_eq!(
            resolve_timezone_offset("+08:00", 0),
            (28800, "+08:00".to_string())
        );
        assert_eq!(
            resolve_timezone_offset("-05:00", 0),
            (-18000, "-05:00".to_string())
        );
    }

    #[test]
    fn test_civil_from_timestamp_accuracy() {
        assert_eq!(civil_from_timestamp(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil_from_timestamp(951827445), (2000, 2, 29, 12, 30, 45));
        assert_eq!(civil_from_timestamp(1789921800), (2026, 9, 20, 16, 30, 0));
        assert_eq!(civil_from_timestamp(1789948800), (2026, 9, 21, 0, 0, 0));
    }

    #[test]
    fn test_civil_with_offset_cross_day() {
        // UTC: 2026-09-24 23:30:00 -> secs = 1790292600
        // With +08:00: local is 2026-09-25 07:30:00
        let utc_secs = 1790292600;
        let offset = 28800; // +8 hours
        assert_eq!(
            civil_from_timestamp_with_offset(utc_secs, offset),
            (2026, 9, 25, 7, 30, 0)
        );
        assert_eq!(format_date_with_tz(utc_secs, offset), "2026-09-25");
        assert_eq!(
            format_datetime_with_tz(utc_secs, offset, "+08:00"),
            "2026-09-25 07:30:00 +08:00"
        );
    }

    #[test]
    fn test_linux_zoneinfo_if_available() {
        if Path::new("/usr/share/zoneinfo/Asia/Taipei").exists() {
            let (offset, abbr) =
                read_linux_zoneinfo("Asia/Taipei", 1790292600).expect("Asia/Taipei zoneinfo");
            assert_eq!(offset, 28800);
            assert_eq!(abbr, "CST");
        }
        if Path::new("/etc/localtime").exists() {
            let res = read_linux_zoneinfo("local", 1790292600);
            assert!(res.is_some());
        }
    }
}
