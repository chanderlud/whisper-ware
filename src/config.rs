use std::cmp::PartialEq;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::time::Duration;

use atomic_float::AtomicF32;
use log::error;
use serde::{Deserialize, Serialize};
use serde_json::{from_reader, to_writer_pretty};
use vst::host::PluginInstance;
use vst::prelude::Plugin;

use crate::error::ErrorKind;
use crate::Result;

#[derive(Serialize, Deserialize, PartialEq)]
struct Config {
    sidechain_hpf: f32,
    input_level: f32,
    sensitivity: f32,
    ratio: f32,
    attack: f32,
    release: f32,
    makeup: f32,
    mix: f32,
    output_level: f32,
    sidechain: f32,
    full_bandwidth: f32,
    input_device: String,
    output_device: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            sidechain_hpf: 20_f32,
            input_level: 1_f32,
            sensitivity: 0.48333332,
            ratio: 1_f32,
            attack: 0_f32,
            release: 0.09090909,
            makeup: 0.33333334,
            mix: 1_f32,
            output_level: 1_f32,
            sidechain: 0_f32,
            full_bandwidth: 1_f32,
            input_device: String::from("Default"),
            output_device: String::from("Default"),
        }
    }
}

pub(crate) struct AtomicConfig {
    sidechain_hpf: AtomicF32,
    input_level: AtomicF32,
    sensitivity: AtomicF32,
    ratio: AtomicF32,
    attack: AtomicF32,
    release: AtomicF32,
    makeup: AtomicF32,
    mix: AtomicF32,
    output_level: AtomicF32,
    sidechain: AtomicF32,
    full_bandwidth: AtomicF32,
    input_device: RwLock<String>,
    output_device: RwLock<String>,
    path: RwLock<PathBuf>,
}

impl From<Config> for AtomicConfig {
    fn from(c: Config) -> Self {
        AtomicConfig {
            sidechain_hpf: AtomicF32::new(c.sidechain_hpf),
            input_level: AtomicF32::new(c.input_level),
            sensitivity: AtomicF32::new(c.sensitivity),
            ratio: AtomicF32::new(c.ratio),
            attack: AtomicF32::new(c.attack),
            release: AtomicF32::new(c.release),
            makeup: AtomicF32::new(c.makeup),
            mix: AtomicF32::new(c.mix),
            output_level: AtomicF32::new(c.output_level),
            sidechain: AtomicF32::new(c.sidechain),
            full_bandwidth: AtomicF32::new(c.full_bandwidth),
            input_device: RwLock::new(c.input_device),
            output_device: RwLock::new(c.output_device),
            path: Default::default(),
        }
    }
}

impl From<Arc<AtomicConfig>> for Config {
    fn from(c: Arc<AtomicConfig>) -> Self {
        Config {
            sidechain_hpf: c.sidechain_hpf.load(Relaxed),
            input_level: c.input_level.load(Relaxed),
            sensitivity: c.sensitivity.load(Relaxed),
            ratio: c.ratio.load(Relaxed),
            attack: c.attack.load(Relaxed),
            release: c.release.load(Relaxed),
            makeup: c.makeup.load(Relaxed),
            mix: c.mix.load(Relaxed),
            output_level: c.output_level.load(Relaxed),
            sidechain: c.sidechain.load(Relaxed),
            full_bandwidth: c.full_bandwidth.load(Relaxed),
            input_device: c.input_device.read().unwrap().clone(),
            output_device: c.output_device.read().unwrap().clone(),
        }
    }
}

impl Default for AtomicConfig {
    fn default() -> Self {
        Config::default().into()
    }
}

impl AtomicConfig {
    /// Creates a new config instance
    pub(crate) fn new(path: PathBuf) -> Result<Self> {
        let serde_config: Config = if path.exists() {
            match File::open(&path) {
                Ok(file) => from_reader(file)?,
                Err(e) => return Err(ErrorKind::ConfigRead(e).into()),
            }
        } else {
            let config = Config::default();

            match File::create(&path) {
                Ok(file) => to_writer_pretty(file, &config)?,
                Err(e) => return Err(ErrorKind::ConfigWrite(e).into()),
            }

            config
        };

        let mut atomic_config: AtomicConfig = serde_config.into();
        atomic_config.path = RwLock::new(path);

        Ok(atomic_config)
    }

    /// Updates the config from another config instance
    pub(crate) fn update_from(&self, other: &Self) -> Result<()> {
        self.sidechain_hpf
            .store(other.sidechain_hpf.load(Relaxed), Relaxed);
        self.input_level
            .store(other.input_level.load(Relaxed), Relaxed);
        self.sensitivity
            .store(other.sensitivity.load(Relaxed), Relaxed);
        self.ratio.store(other.ratio.load(Relaxed), Relaxed);
        self.attack.store(other.attack.load(Relaxed), Relaxed);
        self.release.store(other.release.load(Relaxed), Relaxed);
        self.makeup.store(other.makeup.load(Relaxed), Relaxed);
        self.mix.store(other.mix.load(Relaxed), Relaxed);
        self.output_level
            .store(other.output_level.load(Relaxed), Relaxed);
        self.sidechain.store(other.sidechain.load(Relaxed), Relaxed);
        self.full_bandwidth
            .store(other.full_bandwidth.load(Relaxed), Relaxed);

        let mut input_device = self.input_device.write().map_err(|_| ErrorKind::Poison)?;
        *input_device = other
            .input_device
            .read()
            .map_err(|_| ErrorKind::Poison)?
            .clone();

        let mut output_device = self.output_device.write().map_err(|_| ErrorKind::Poison)?;
        *output_device = other
            .output_device
            .read()
            .map_err(|_| ErrorKind::Poison)?
            .clone();

        let mut path = self.path.write().map_err(|_| ErrorKind::Poison)?;
        *path = other.path.read().map_err(|_| ErrorKind::Poison)?.clone();

        Ok(())
    }

    /// Returns the input and output devices
    pub(crate) fn devices(&self) -> Result<(String, String)> {
        let input_device = self
            .input_device
            .read()
            .map_err(|_| ErrorKind::Poison)?
            .clone();

        let output_device = self
            .output_device
            .read()
            .map_err(|_| ErrorKind::Poison)?
            .clone();

        Ok((input_device, output_device))
    }

    /// Sets the input device
    pub(crate) fn set_input_device(&self, device: String) -> Result<()> {
        let mut input_device = self.input_device.write().map_err(|_| ErrorKind::Poison)?;
        *input_device = device;
        Ok(())
    }

    /// Sets the output device
    pub(crate) fn set_output_device(&self, device: String) -> Result<()> {
        let mut output_device = self.output_device.write().map_err(|_| ErrorKind::Poison)?;
        *output_device = device;
        Ok(())
    }

    /// Applies the parameters to the VST plugin
    pub(crate) fn apply_parameters(&self, instance: &mut PluginInstance) {
        let parameters = instance.get_parameter_object();

        parameters.set_parameter(0, self.sidechain_hpf.load(Relaxed));
        parameters.set_parameter(1, self.input_level.load(Relaxed));
        parameters.set_parameter(2, self.sensitivity.load(Relaxed));
        parameters.set_parameter(3, self.ratio.load(Relaxed));
        parameters.set_parameter(4, self.attack.load(Relaxed));
        parameters.set_parameter(5, self.release.load(Relaxed));
        parameters.set_parameter(6, self.makeup.load(Relaxed));
        parameters.set_parameter(7, self.mix.load(Relaxed));
        parameters.set_parameter(8, self.output_level.load(Relaxed));
        parameters.set_parameter(9, self.sidechain.load(Relaxed));
        parameters.set_parameter(10, self.full_bandwidth.load(Relaxed));
    }

    /// Called when a parameter is changed in the VST plugin
    pub(crate) fn set_parameter(&self, index: usize, value: f32) {
        let atomic = match index {
            0 => &self.sidechain_hpf,
            1 => &self.input_level,
            2 => &self.sensitivity,
            3 => &self.ratio,
            4 => &self.attack,
            5 => &self.release,
            6 => &self.makeup,
            7 => &self.mix,
            8 => &self.output_level,
            9 => &self.sidechain,
            10 => &self.full_bandwidth,
            _ => {
                error!("Invalid parameter index");
                return;
            }
        };

        atomic.store(value, Relaxed);
    }

    /// Allows the config to save without blocking the main thread or spamming the disk
    pub(crate) fn save_thread(self: &Arc<Self>) -> Result<()> {
        let interval = Duration::from_millis(300);
        let mut old_config: Config = self.clone().into();

        loop {
            sleep(interval);

            let serde_config = self.clone().into();

            if old_config == serde_config {
                continue;
            } else {
                old_config = serde_config;
            }

            let result = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&*self.path.read().map_err(|_| ErrorKind::Poison)?);

            match result {
                Ok(mut file) => {
                    to_writer_pretty(&mut file, &old_config)?;
                }
                Err(error) => {
                    error!("Failed to save config file: {}", error);
                }
            }
        }
    }
}
