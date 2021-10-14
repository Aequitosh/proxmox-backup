//! # Proxmox RRD format version 2
//!
//! The new format uses
//! [CBOR](https://datatracker.ietf.org/doc/html/rfc8949) as storage
//! format. This way we can use the serde serialization framework,
//! which make our code more flexible, much nicer and type safe.
//!
//! ## Features
//!
//! * Well defined data format [CBOR](https://datatracker.ietf.org/doc/html/rfc8949)
//! * Plattform independent (big endian f64, hopefully a standard format?)
//! * Arbitrary number of RRAs (dynamically changeable)

use std::path::Path;

use anyhow::{bail, format_err, Error};

use serde::{Serialize, Deserialize};

use proxmox::tools::fs::{replace_file, CreateOptions};
use proxmox_schema::api;

use crate::rrd_v1;

/// Proxmox RRD v2 file magic number
// openssl::sha::sha256(b"Proxmox Round Robin Database file v2.0")[0..8];
pub const PROXMOX_RRD_MAGIC_2_0: [u8; 8] = [224, 200, 228, 27, 239, 112, 122, 159];

#[api()]
#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
/// RRD data source type
pub enum DST {
    /// Gauge values are stored unmodified.
    Gauge,
    /// Stores the difference to the previous value.
    Derive,
    /// Stores the difference to the previous value (like Derive), but
    /// detect counter overflow (and ignores that value)
    Counter,
}

#[api()]
#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
/// Consolidation function
pub enum CF {
    /// Average
    Average,
    /// Maximum
    Maximum,
    /// Minimum
    Minimum,
    /// Use the last value
    Last,
}

#[derive(Serialize, Deserialize)]
/// Data source specification
pub struct DataSource {
    /// Data source type
    pub dst: DST,
    /// Last update time (epoch)
    pub last_update: f64,
    /// Stores the last value, used to compute differential value for
    /// derive/counters
    pub last_value: f64,
}

impl DataSource {

    pub fn new(dst: DST) -> Self {
        Self {
            dst,
            last_update: 0.0,
            last_value: f64::NAN,
        }
    }

    fn compute_new_value(&mut self, time: f64, mut value: f64) -> Result<f64, Error> {
        if time < 0.0 {
            bail!("got negative time");
        }
        if time <= self.last_update {
            bail!("time in past ({} < {})", time, self.last_update);
        }

        if value.is_nan() {
            bail!("new value is NAN");
        }

        // derive counter value
        let is_counter = self.dst == DST::Counter;

        if is_counter || self.dst == DST::Derive {
            let time_diff = time - self.last_update;

            let diff = if self.last_value.is_nan() {
                0.0
            } else if is_counter && value < 0.0 {
                bail!("got negative value for counter");
            } else if is_counter && value < self.last_value {
                // Note: We do not try automatic overflow corrections, but
                // we update last_value anyways, so that we can compute the diff
                // next time.
                self.last_value = value;
                bail!("conter overflow/reset detected");
            } else {
                value - self.last_value
            };
            self.last_value = value;
            value = diff/time_diff;
        } else {
            self.last_value = value;
        }

        Ok(value)
    }


}

#[derive(Serialize, Deserialize)]
/// Round Robin Archive
pub struct RRA {
    /// Number of seconds spaned by a single data entry.
    pub resolution: u64,
    /// Consolitation function.
    pub cf: CF,
    /// Count values computed inside this update interval.
    pub last_count: u64,
    /// The actual data entries.
    pub data: Vec<f64>,
}

impl RRA {

    pub fn new(cf: CF, resolution: u64, points: usize) -> Self {
        Self {
            cf,
            resolution,
            last_count: 0,
            data: vec![f64::NAN; points],
        }
    }

    pub fn slot_end_time(&self, time: u64) -> u64 {
        self.resolution * (time / self.resolution + 1)
    }

    pub fn slot_start_time(&self, time: u64) -> u64 {
        self.resolution * (time / self.resolution)
    }

    pub fn slot(&self, time: u64) -> usize {
        ((time / self.resolution) as usize) % self.data.len()
    }

    // directly overwrite data slots
    // the caller need to set last_update value on the DataSource manually.
    pub fn insert_data(
        &mut self,
        start: u64,
        resolution: u64,
        data: Vec<Option<f64>>,
    ) -> Result<(), Error> {
        if resolution != self.resolution {
            bail!("inser_data failed: got wrong resolution");
        }

        let mut index = self.slot(start);

        for i in 0..data.len() {
            if let Some(v) = data[i] {
                self.data[index] = v;
            }
            index += 1; if index >= self.data.len() { index = 0; }
        }
        Ok(())
    }

    fn delete_old_slots(&mut self, time: f64, last_update: f64) {
        let epoch = time as u64;
        let last_update = last_update as u64;
        let reso = self.resolution;
        let num_entries = self.data.len() as u64;

        let min_time = epoch.saturating_sub(num_entries*reso);
        let min_time = (min_time/reso + 1)*reso;
        let mut t = last_update.saturating_sub(num_entries*reso);

        let mut index = self.slot(t);

        for _ in 0..num_entries {
            t += reso;
            index += 1; if index >= self.data.len() { index = 0; }
            if t < min_time {
                self.data[index] = f64::NAN;
            } else {
                break;
            }
        }
    }

    fn compute_new_value(&mut self, time: f64, last_update: f64, value: f64) {
        let epoch = time as u64;
        let last_update = last_update as u64;
        let reso = self.resolution;

        let index = self.slot(epoch);
        let last_index = self.slot(last_update);

        if (epoch - last_update) > reso || index != last_index {
            self.last_count = 0;
        }

        let last_value = self.data[index];
        if last_value.is_nan() {
            self.last_count = 0;
        }

        let new_count = if self.last_count < u64::MAX {
            self.last_count + 1
        } else {
            u64::MAX // should never happen
        };

        if self.last_count == 0 {
            self.data[index] = value;
            self.last_count = 1;
        } else {
            let new_value = match self.cf {
                CF::Maximum => if last_value > value { last_value } else { value },
                CF::Minimum => if last_value < value { last_value } else { value },
                CF::Last => value,
                CF::Average => {
                    (last_value*(self.last_count as f64))/(new_count as f64)
                        + value/(new_count as f64)
                }
            };
            self.data[index] = new_value;
            self.last_count = new_count;
        }
    }

    pub fn extract_data(
        &self,
        start: u64,
        end: u64,
        last_update: f64,
    ) -> (u64, u64, Vec<Option<f64>>) {
        let last_update = last_update as u64;
        let reso = self.resolution;
        let num_entries = self.data.len() as u64;

        let mut list = Vec::new();

        let rrd_end = self.slot_end_time(last_update);
        let rrd_start = rrd_end.saturating_sub(reso*num_entries);

        let mut t = start;
        let mut index = self.slot(t);
        for _ in 0..num_entries {
            if t > end { break; };
            if t < rrd_start || t >= rrd_end {
                list.push(None);
            } else {
                let value = self.data[index];
                if value.is_nan() {
                    list.push(None);
                } else {
                    list.push(Some(value));
                }
            }
            t += reso;
            index += 1; if index >= self.data.len() { index = 0; }
        }

        (start, reso, list)
    }
}

#[derive(Serialize, Deserialize)]
/// Round Robin Database
pub struct RRD {
    /// The data source definition
    pub source: DataSource,
    /// List of round robin archives
    pub rra_list: Vec<RRA>,
}

impl RRD {

    pub fn new(dst: DST, rra_list: Vec<RRA>) -> RRD {

        let source = DataSource::new(dst);

        RRD {
            source,
            rra_list,
        }

    }

    fn from_raw(raw: &[u8]) -> Result<Self, Error> {
        if raw.len() < 8 {
            bail!("not an rrd file - file is too small ({})", raw.len());
        }

        let rrd = if raw[0..8] == rrd_v1::PROXMOX_RRD_MAGIC_1_0 {
            let v1 = rrd_v1::RRDv1::from_raw(&raw)?;
            v1.to_rrd_v2()
                .map_err(|err| format_err!("unable to convert from old V1 format - {}", err))?
        } else if raw[0..8] == PROXMOX_RRD_MAGIC_2_0 {
            serde_cbor::from_slice(&raw[8..])
                .map_err(|err| format_err!("unable to decode RRD file - {}", err))?
        } else {
            bail!("not an rrd file - unknown magic number");
        };

        if rrd.source.last_update < 0.0 {
            bail!("rrd file has negative last_update time");
        }

        Ok(rrd)
    }

    /// Load data from a file
    pub fn load(path: &Path) -> Result<Self, std::io::Error> {
        let raw = std::fs::read(path)?;

        match Self::from_raw(&raw) {
            Ok(rrd) => Ok(rrd),
            Err(err) =>  Err(std::io::Error::new(std::io::ErrorKind::Other, err.to_string())),
        }
    }

    /// Store data into a file (atomic replace file)
    pub fn save(&self, filename: &Path, options: CreateOptions) -> Result<(), Error> {
        let mut data: Vec<u8> = Vec::new();
        data.extend(&PROXMOX_RRD_MAGIC_2_0);
        serde_cbor::to_writer(&mut data, self)?;
        replace_file(filename, &data, options)
    }

    pub fn last_update(&self) -> f64 {
        self.source.last_update
    }

    /// Update the value (in memory)
    ///
    /// Note: This does not call [Self::save].
    pub fn update(&mut self, time: f64, value: f64) {

        let value = match self.source.compute_new_value(time, value) {
            Ok(value) => value,
            Err(err) => {
                log::error!("rrd update failed: {}", err);
                return;
            }
        };

        let last_update = self.source.last_update;
        self.source.last_update = time;

        for rra in self.rra_list.iter_mut() {
            rra.delete_old_slots(time, last_update);
            rra.compute_new_value(time, last_update, value);
        }
    }

    /// Extract data from the archive
    ///
    /// This selects the RRA with specified [CF] and (minimum)
    /// resolution, and extract data from `start` to `end`.
    ///
    /// `start`: Start time. If not sepecified, we simply extract 10 data points.
    /// `end`: End time. Default is to use the current time.
    pub fn extract_data(
        &self,
        cf: CF,
        resolution: u64,
        start: Option<u64>,
        end: Option<u64>,
    ) -> Result<(u64, u64, Vec<Option<f64>>), Error> {

        let mut rra: Option<&RRA> = None;
        for item in self.rra_list.iter() {
            if item.cf != cf { continue; }
            if item.resolution > resolution { continue; }

            if let Some(current) = rra {
                if item.resolution > current.resolution {
                    rra = Some(item);
                }
            } else {
                rra = Some(item);
            }
        }

        match rra {
            Some(rra) => {
                let end = end.unwrap_or_else(|| proxmox_time::epoch_f64() as u64);
                let start = start.unwrap_or(end - 10*rra.resolution);
                Ok(rra.extract_data(start, end, self.source.last_update))
            }
            None => bail!("unable to find RRA suitable ({:?}:{})", cf, resolution),
        }
    }

}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_rra_maximum_gauge_test() -> Result<(), Error> {
        let rra = RRA::new(CF::Maximum, 60, 5);
        let mut rrd = RRD::new(DST::Gauge, vec![rra]);

        for i in 2..10 {
            rrd.update((i as f64)*30.0, i as f64);
        }

        let (start, reso, data) = rrd.extract_data(CF::Maximum, 60, Some(0), Some(5*60))?;
        assert_eq!(start, 0);
        assert_eq!(reso, 60);
        assert_eq!(data, [None, Some(3.0), Some(5.0), Some(7.0), Some(9.0)]);

        Ok(())
    }

    #[test]
    fn basic_rra_minimum_gauge_test() -> Result<(), Error> {
        let rra = RRA::new(CF::Minimum, 60, 5);
        let mut rrd = RRD::new(DST::Gauge, vec![rra]);

        for i in 2..10 {
            rrd.update((i as f64)*30.0, i as f64);
        }

        let (start, reso, data) = rrd.extract_data(CF::Minimum, 60, Some(0), Some(5*60))?;
        assert_eq!(start, 0);
        assert_eq!(reso, 60);
        assert_eq!(data, [None, Some(2.0), Some(4.0), Some(6.0), Some(8.0)]);

        Ok(())
    }

    #[test]
    fn basic_rra_last_gauge_test() -> Result<(), Error> {
        let rra = RRA::new(CF::Last, 60, 5);
        let mut rrd = RRD::new(DST::Gauge, vec![rra]);

        for i in 2..10 {
            rrd.update((i as f64)*30.0, i as f64);
        }

        assert!(rrd.extract_data(CF::Average, 60, Some(0), Some(5*60)).is_err(), "CF::Average should not exist");

        let (start, reso, data) = rrd.extract_data(CF::Last, 60, Some(0), Some(20*60))?;
        assert_eq!(start, 0);
        assert_eq!(reso, 60);
        assert_eq!(data, [None, Some(3.0), Some(5.0), Some(7.0), Some(9.0)]);

        Ok(())
    }

    #[test]
    fn basic_rra_average_derive_test() -> Result<(), Error> {
        let rra = RRA::new(CF::Average, 60, 5);
        let mut rrd = RRD::new(DST::Derive, vec![rra]);

        for i in 2..10 {
            rrd.update((i as f64)*30.0, (i*60) as f64);
        }

        let (start, reso, data) = rrd.extract_data(CF::Average, 60, Some(60), Some(5*60))?;
        assert_eq!(start, 60);
        assert_eq!(reso, 60);
        assert_eq!(data, [Some(1.0), Some(2.0), Some(2.0), Some(2.0), None]);

        Ok(())
    }

    #[test]
    fn basic_rra_average_gauge_test() -> Result<(), Error> {
        let rra = RRA::new(CF::Average, 60, 5);
        let mut rrd = RRD::new(DST::Gauge, vec![rra]);

        for i in 2..10 {
            rrd.update((i as f64)*30.0, i as f64);
        }

        let (start, reso, data) = rrd.extract_data(CF::Average, 60, Some(60), Some(5*60))?;
        assert_eq!(start, 60);
        assert_eq!(reso, 60);
        assert_eq!(data, [Some(2.5), Some(4.5), Some(6.5), Some(8.5), None]);

        for i in 10..14 {
            rrd.update((i as f64)*30.0, i as f64);
        }

        let (start, reso, data) = rrd.extract_data(CF::Average, 60, Some(60), Some(5*60))?;
        assert_eq!(start, 60);
        assert_eq!(reso, 60);
        assert_eq!(data, [None, Some(4.5), Some(6.5), Some(8.5), Some(10.5)]);

        let (start, reso, data) = rrd.extract_data(CF::Average, 60, Some(3*60), Some(8*60))?;
        assert_eq!(start, 3*60);
        assert_eq!(reso, 60);
        assert_eq!(data, [Some(6.5), Some(8.5), Some(10.5), Some(12.5), None]);

        // add much newer vaule (should delete all previous/outdated value)
        let i = 100; rrd.update((i as f64)*30.0, i as f64);
        println!("TEST {:?}", serde_json::to_string_pretty(&rrd));

        let (start, reso, data) = rrd.extract_data(CF::Average, 60, Some(100*30), Some(100*30 + 5*60))?;
        assert_eq!(start, 100*30);
        assert_eq!(reso, 60);
        assert_eq!(data, [Some(100.0), None, None, None, None]);

        // extract with end time smaller than start time
        let (start, reso, data) = rrd.extract_data(CF::Average, 60, Some(100*30), Some(60))?;
        assert_eq!(start, 100*30);
        assert_eq!(reso, 60);
        assert_eq!(data, []);

        Ok(())
    }
}
