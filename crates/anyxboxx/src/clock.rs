//! `time.strftime("%Y-%m-%d_%H%M%S")`: the name of a report folder, in local time on Windows
//! (UTC elsewhere: the standard library has no time zone).

/// Calendar date and time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Moment {
    pub fn stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}_{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// From seconds since 1970-01-01 UTC (Howard Hinnant's `civil_from_days`).
    pub fn from_unix(seconds: u64) -> Self {
        const DAY: u64 = 86_400;
        let (days, time) = ((seconds / DAY) as i64, seconds % DAY);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        Self {
            year: yoe + era * 400 + i64::from(month <= 2),
            month,
            day,
            hour: (time / 3_600) as u32,
            minute: (time % 3_600 / 60) as u32,
            second: (time % 60) as u32,
        }
    }
}

#[cfg(windows)]
fn now() -> Moment {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;

    // SAFETY: GetLocalTime only writes the SYSTEMTIME it is given.
    let time: SYSTEMTIME = unsafe {
        let mut time = std::mem::zeroed();
        GetLocalTime(&mut time);
        time
    };
    Moment {
        year: i64::from(time.wYear),
        month: u32::from(time.wMonth),
        day: u32::from(time.wDay),
        hour: u32::from(time.wHour),
        minute: u32::from(time.wMinute),
        second: u32::from(time.wSecond),
    }
}

#[cfg(not(windows))]
fn now() -> Moment {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Moment::from_unix(seconds)
}

pub fn stamp() -> String {
    now().stamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_seconds_become_calendar_dates() {
        assert_eq!(Moment::from_unix(0).stamp(), "1970-01-01_000000");
        assert_eq!(Moment::from_unix(951_782_400).stamp(), "2000-02-29_000000");
        assert_eq!(
            Moment::from_unix(1_795_626_930).stamp(),
            "2026-11-25_171530"
        );
    }

    #[test]
    fn the_current_stamp_has_the_python_shape() {
        let stamp = stamp();
        assert_eq!(stamp.len(), "2026-11-25_141530".len());
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "_");
    }
}
