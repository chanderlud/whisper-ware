use crate::Result;
use atomic_float::AtomicF32;
use kanal::{Receiver, Sender};
use log::error;
use serde::{Deserialize, Serialize};
use serde_json::{from_slice, to_writer_pretty};
use std::cmp::PartialEq;
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vst::host::PluginInstance;
use vst::prelude::Plugin;

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

impl Config {
    fn atomic(&self, path: PathBuf, notify: Sender<()>) -> AtomicConfig {
        AtomicConfig {
            sidechain_hpf: AtomicF32::new(self.sidechain_hpf),
            input_level: AtomicF32::new(self.input_level),
            sensitivity: AtomicF32::new(self.sensitivity),
            ratio: AtomicF32::new(self.ratio),
            attack: AtomicF32::new(self.attack),
            release: AtomicF32::new(self.release),
            makeup: AtomicF32::new(self.makeup),
            mix: AtomicF32::new(self.mix),
            output_level: AtomicF32::new(self.output_level),
            sidechain: AtomicF32::new(self.sidechain),
            full_bandwidth: AtomicF32::new(self.full_bandwidth),
            input_device: Mutex::new(self.input_device.clone()),
            output_device: Mutex::new(self.output_device.clone()),
            path,
            dirty: Default::default(),
            notify,
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
    input_device: Mutex<String>,
    output_device: Mutex<String>,
    path: PathBuf,
    dirty: AtomicBool,
    notify: Sender<()>,
}

impl AtomicConfig {
    const PARAM_COUNT: usize = 11;

    /// Creates a new config instance, panics if I/O fails
    pub(crate) fn new(notify: Sender<()>) -> Self {
        let config_dir = dirs::config_dir().unwrap().join("WhisperWare");
        if !config_dir.exists() {
            create_dir_all(&config_dir).unwrap();
        }
        let config_path = config_dir.join("config.json");

        let mut config_option: Option<Config> = None;
        if config_path.exists() {
            let mut file = File::open(&config_path).unwrap();
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).unwrap();

            // if the file contains invalid data, fall back to default config
            if let Ok(config) = from_slice(&buffer) {
                config_option = Some(config);
            }
        }

        config_option
            .unwrap_or_default()
            .atomic(config_path, notify)
    }

    /// Returns the input and output devices
    pub(crate) fn devices(&self) -> (String, String) {
        let input_device = self.input_device.lock().unwrap().clone();
        let output_device = self.output_device.lock().unwrap().clone();
        (input_device, output_device)
    }

    /// Sets the input device
    pub(crate) fn set_input_device(&self, device: String) -> Result<()> {
        let mut input_device = self.input_device.lock().unwrap();
        *input_device = device;
        self.mark_dirty();
        Ok(())
    }

    /// Sets the output device
    pub(crate) fn set_output_device(&self, device: String) -> Result<()> {
        let mut output_device = self.output_device.lock().unwrap();
        *output_device = device;
        self.mark_dirty();
        Ok(())
    }

    /// Applies the parameters to the VST plugin
    pub(crate) fn apply_parameters(&self, instance: &mut PluginInstance) {
        let parameters = instance.get_parameter_object();

        for index in 0..Self::PARAM_COUNT {
            if let Some(a) = self.param_atomic(index) {
                parameters.set_parameter(index as i32, a.load(Relaxed));
            }
        }
    }

    /// Called when a parameter is changed in the VST plugin
    pub(crate) fn set_parameter(&self, index: usize, value: f32) {
        if let Some(a) = self.param_atomic(index) {
            a.store(value, Relaxed);
            self.mark_dirty();
        } else {
            error!("Invalid parameter index: {}", index);
        }
    }

    /// Returns the current state of the config as Config
    fn snapshot(&self) -> Config {
        Config {
            sidechain_hpf: self.sidechain_hpf.load(Relaxed),
            input_level: self.input_level.load(Relaxed),
            sensitivity: self.sensitivity.load(Relaxed),
            ratio: self.ratio.load(Relaxed),
            attack: self.attack.load(Relaxed),
            release: self.release.load(Relaxed),
            makeup: self.makeup.load(Relaxed),
            mix: self.mix.load(Relaxed),
            output_level: self.output_level.load(Relaxed),
            sidechain: self.sidechain.load(Relaxed),
            full_bandwidth: self.full_bandwidth.load(Relaxed),
            input_device: self.input_device.lock().unwrap().clone(),
            output_device: self.output_device.lock().unwrap().clone(),
        }
    }

    /// Notifies writer that the config has changed
    fn mark_dirty(&self) {
        // avoid spamming the channel on repeated writes
        if !self.dirty.swap(true, Relaxed) {
            let _ = self.notify.send(());
        }
    }

    /// Returns the atomic backer for an index in the plugin parameters
    fn param_atomic(&self, index: usize) -> Option<&AtomicF32> {
        match index {
            0 => Some(&self.sidechain_hpf),
            1 => Some(&self.input_level),
            2 => Some(&self.sensitivity),
            3 => Some(&self.ratio),
            4 => Some(&self.attack),
            5 => Some(&self.release),
            6 => Some(&self.makeup),
            7 => Some(&self.mix),
            8 => Some(&self.output_level),
            9 => Some(&self.sidechain),
            10 => Some(&self.full_bandwidth),
            _ => None,
        }
    }
}

/// saves the config without blocking the main thread or spamming the disk
pub(crate) fn config_saver(config: Arc<AtomicConfig>, receiver: Receiver<()>) -> Result<()> {
    let interval = Duration::from_millis(200); // debounce window

    loop {
        // block until at least one change arrives
        if receiver.recv().is_err() {
            return Ok(());
        }

        // keep draining events for a short time
        while receiver.recv_timeout(interval).is_ok() {
            // do nothing, just coalesce
        }

        // snapshot and write once per burst of changes
        config.dirty.store(false, Relaxed);
        let cfg = config.snapshot();

        let result = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&config.path);

        match result {
            Ok(mut file) => {
                to_writer_pretty(&mut file, &cfg)?;
            }
            Err(error) => {
                error!("Failed to save config file: {}", error);
            }
        }
    }
}
