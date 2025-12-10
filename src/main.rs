#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, default_host};
use kanal::{Receiver, Sender, bounded, unbounded};
use lazy_static::lazy_static;
use log::{LevelFilter, debug, error, info, warn};
use minimal_windows_gui as win;
use minimal_windows_gui::class::Class;
use minimal_windows_gui::message::Message;
use minimal_windows_gui::window::Window;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{sleep, spawn};
use std::time::Duration;
use tray_icon::menu::{MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder, menu::Menu};
use vst::host::{Host, HostBuffer, PluginInstance, PluginLoader};
use vst::prelude::Plugin;
use winapi::shared::minwindef::LPARAM;
use winapi::shared::windef::HWND;
use winapi::um::processthreadsapi::GetCurrentProcess;
use winapi::um::processthreadsapi::SetPriorityClass;
use winapi::um::winbase::HIGH_PRIORITY_CLASS;
use winapi::um::winuser::{
    LB_GETCURSEL, LB_GETTEXT, LB_GETTEXTLEN, LB_SETCURSEL, SW_HIDE, SW_SHOW, SendMessageA,
    ShowWindow, UpdateWindow,
};

use crate::config::{AtomicConfig, config_saver};
use crate::device_callback::wait_for_audio_device_change;
use crate::error::ErrorKind;

// block non windows builds
#[cfg(not(target_os = "windows"))]
compile_error!("This application only supports Windows.");

mod config;
mod device_callback;
mod error;

type Result<T> = std::result::Result<T, error::Error>;

/// the size of the audio frames used for processing
const BLOCK_SIZE: usize = 512;
/// the class name for windowing
const CLASS_NAME: &str = "whisperWare";
/// the control ids for the device manager
const IDC_INPUT_SELECT: u16 = 101;
const IDC_OUTPUT_SELECT: u16 = 102;
const SILENCE: [f32; 2] = [0_f32, 0_f32];

// shared values accessed in callbacks
lazy_static! {
    static ref INPUT_DEVICES: RwLock<Vec<String>> = Default::default();
    static ref OUTPUT_DEVICES: RwLock<Vec<String>> = Default::default();
    static ref CONFIG: Arc<AtomicConfig> = {
        let (sender, receiver) = unbounded();
        let config = Arc::new(AtomicConfig::new(sender));
        let config_clone = Arc::clone(&config);
        spawn(move || config_saver(config_clone, receiver));
        config
    };
}

/// the host for the compressor plugin
struct CompressorHost;

impl Host for CompressorHost {
    /// callback for parameter changes
    fn automate(&self, index: i32, value: f32) {
        CONFIG.set_parameter(index as usize, value);
    }
}

fn main() -> Result<()> {
    simple_logging::log_to_file("whisper_ware.log", LevelFilter::Warn)?;
    log_panics::init();

    unsafe {
        let process = GetCurrentProcess();

        if SetPriorityClass(process, HIGH_PRIORITY_CLASS) == 0 {
            warn!("Failed to set process priority");
        } else {
            info!("Process priority set to high");
        }
    }

    if let Err(error) = app() {
        win::messagebox::message_box(
            "Whisper Ware encountered a critical error",
            &error.to_string(),
            &[win::messagebox::Config::IconError],
        )?;
    }

    Ok(())
}

/// the main application logic
fn app() -> Result<()> {
    let class = Arc::new(
        win::class::build()
            .load_icon(win::icon::Icon::FromResource(1))?
            .background(win::class::Background::Window)
            .load_small_icon(win::icon::Icon::FromResource(1))?
            .register(CLASS_NAME)?,
    );

    // create the host
    let plugin_host = Arc::new(Mutex::new(CompressorHost));
    // initialize the plugin loader
    let mut loader = PluginLoader::load(Path::new("RoughRider3.dll"), plugin_host)?;

    // create the plugin instance
    let mut instance = loader.instance()?;
    CONFIG.apply_parameters(&mut instance); // apply the saved parameters

    // get the editor
    let mut editor = instance.get_editor().ok_or(ErrorKind::EditorMissing)?;

    let editor_window = win::window::build()
        .set_message_callback(editor_callback)
        .add_extended_style(win::window::ExtendedStyle::ClientEdge)
        .add_style(win::window::Style::OverlappedWindow)
        .add_style(win::window::Style::Caption)
        .add_style(win::window::Style::SysMenu)
        .add_style(win::window::Style::Group)
        .size(1125, 410)
        .create(&class, "Configurator")?;

    let editor_hwnd = editor_window.hwnd_ptr() as usize;

    // load the icon from the resources
    let icon = Icon::from_resource(1, None)?;

    // create the tray menu
    let configurator = MenuItem::new("Show Configurator", true, None);
    let device_manager = MenuItem::new("Device Manager", true, None);
    let view_log = MenuItem::new("View Log", true, None);
    let restart_backend = MenuItem::new("Restart Backend", true, None);
    let exit = MenuItem::new("Exit", true, None);

    let tray_menu = Menu::with_items(&[
        &configurator,
        &device_manager,
        &restart_backend,
        &view_log,
        &exit,
    ])?;

    // create the tray icon
    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_icon(icon)
        .build()?;

    // controls the background audio processing thread
    let run: Arc<AtomicBool> = Arc::new(AtomicBool::new(true));
    // prevents multiple instances of the device manager from opening
    let manager_open: Arc<AtomicBool> = Default::default();
    // the host for the audio recording and playback
    let cpal_host = Arc::new(default_host());

    // references for the menu event handler
    let run_clone = Arc::clone(&run);
    let host_clone = Arc::clone(&cpal_host);
    let class_clone = Arc::clone(&class);

    MenuEvent::set_event_handler(Some(Box::new(move |event: MenuEvent| {
        let result = menu_handler(
            event,
            editor_hwnd,
            &manager_open,
            &run_clone,
            &host_clone,
            &class_clone,
        );

        if let Err(error) = result {
            error!("an error occurred in the menu handler: {}", error);
        }
    })));

    // references for the backend thread
    let run_clone = Arc::clone(&run);

    spawn(move || {
        // only allows the plugin to be initialized once
        let mut initialize = true;
        // only log each error once
        let mut last_error: Option<ErrorKind> = None;

        loop {
            match backend(&cpal_host, &mut instance, &run_clone, &mut initialize) {
                Ok(()) => (),
                Err(error) => match error.kind {
                    ErrorKind::NoInputDevice | ErrorKind::NoOutputDevice => {
                        debug!("waiting for audio device change");
                        wait_for_audio_device_change();
                        debug!("audio device change occurred");
                        continue;
                    }
                    error => {
                        if last_error.as_ref() != Some(&error) {
                            error!("backend error: {error:?}");
                        }
                        last_error = Some(error);
                    }
                },
            }

            sleep(Duration::from_millis(100));
        }
    });

    // open the editor
    editor.open(editor_hwnd as *mut std::ffi::c_void);
    // run the event loop for the editor window
    win::message_loop();

    Ok(())
}

/// configures and runs the audio processing backend
fn backend(
    host: &Arc<cpal::Host>,
    instance: &mut PluginInstance,
    run: &Arc<AtomicBool>,
    initialize: &mut bool,
) -> Result<()> {
    let (input_device_name, output_device_name) = CONFIG.devices();
    let mut input_devices = host.input_devices()?;
    let mut output_devices = host.output_devices()?;

    let input_device = if input_device_name == "Default" {
        host.default_input_device()
            .ok_or(ErrorKind::NoInputDevice)?
    } else {
        input_devices
            .find(|device| device_by_name(device, &input_device_name))
            .ok_or(ErrorKind::NoInputDevice)?
    };

    let output_device = if output_device_name == "Default" {
        host.default_output_device()
            .ok_or(ErrorKind::NoOutputDevice)?
    } else {
        output_devices
            .find(|device| device_by_name(device, &output_device_name))
            .ok_or(ErrorKind::NoOutputDevice)?
    };

    info!("output device: {:?}", output_device.name());
    info!("input device: {:?}", input_device.name());

    let input_config = input_device.default_input_config()?;
    let output_config = output_device.default_output_config()?;
    let input_sample_rate = input_config.sample_rate().0 as f32;
    let output_sample_rate = output_config.sample_rate().0 as f32;
    let input_channels = input_config.channels() as usize;
    let output_channels = output_config.channels() as usize;

    if input_sample_rate != output_sample_rate {
        Err(ErrorKind::InvalidConfiguration(
            "input and output sample rates are different",
        ))?;
    } else if input_channels != 2 || output_channels != 2 {
        Err(ErrorKind::InvalidConfiguration("only stereo is supported"))?;
    }

    instance.set_sample_rate(input_sample_rate);
    instance.set_block_size(BLOCK_SIZE as i64);

    if *initialize {
        instance.init();
        *initialize = false;
    }

    // the input to processor receiver
    let (input_sender, input_receiver) = bounded::<[f32; 2]>(BLOCK_SIZE * 4);
    // the processor to output sender
    let (output_sender, output_receiver) = bounded::<[f32; 2]>(BLOCK_SIZE * 4);
    // allows input_stream to stop the program on errors
    let run_clone_a = Arc::clone(run);
    // allows output_stream to stop the program on errors
    let run_clone_b = Arc::clone(run);

    let input_stream = input_device.build_input_stream(
        &input_config.clone().into(),
        move |input: &[f32], _: &_| {
            for frame in input.chunks(2) {
                _ = input_sender.try_send([frame[0], frame[1]]);
            }
        },
        move |error| {
            error!("an error occurred on the input stream: {error}");
            run_clone_a.store(false, Relaxed);
        },
        None,
    )?;

    let output_stream = output_device.build_output_stream(
        &output_config.clone().into(),
        move |output: &mut [f32], _: &_| {
            for frame in output.chunks_mut(2) {
                let samples = output_receiver.recv().unwrap_or(SILENCE);
                frame[0] = samples[0];
                frame[1] = samples[1];
            }
        },
        move |error| {
            error!("an error occurred on the output stream: {error}");
            run_clone_b.store(false, Relaxed);
        },
        None,
    )?;

    input_stream.play()?;
    output_stream.play()?;

    processor(input_receiver, output_sender, instance, run)
}

/// the audio processing thread
fn processor(
    receiver: Receiver<[f32; 2]>,
    sender: Sender<[f32; 2]>,
    instance: &mut PluginInstance,
    run: &Arc<AtomicBool>,
) -> Result<()> {
    // buffers for the audio processing
    // three inputs/outputs are needed for stereo processing
    let mut inputs = [[0_f32; BLOCK_SIZE]; 3];
    let mut outputs = [[0_f32; BLOCK_SIZE]; 3];
    // the host buffer
    let mut buffer = HostBuffer::new(3, 3);
    // the current position in the input buffers
    let mut position = 0;

    while run.load(Relaxed) {
        let frame = receiver.recv()?;

        // deinterleave the input
        inputs[0][position] = frame[0];
        inputs[1][position] = frame[1];
        position += 1; // advance the position in the buffer

        if position < BLOCK_SIZE {
            // if the buffer is not full, continue
            continue;
        } else {
            // reset the position
            position = 0;
        }

        // bind the buffer to the inputs and outputs
        let mut audio_buffer = buffer.bind(&inputs, &mut outputs);
        // process the audio
        instance.process(&mut audio_buffer);

        // re-interleave the processed buffers and send it to the output
        for frame in outputs[0].into_iter().zip(outputs[1].into_iter()) {
            sender.try_send([frame.0, frame.1])?;
        }
    }

    // restore original state
    run.store(true, Relaxed);
    Ok(())
}

/// menu event handler for tray application
fn menu_handler(
    event: MenuEvent,
    editor_hwnd: usize,
    manager_open: &Arc<AtomicBool>,
    run_clone: &Arc<AtomicBool>,
    host_clone: &Arc<cpal::Host>,
    class_clone: &Arc<Class>,
) -> Result<()> {
    match event.id.as_ref().parse::<i32>() {
        Ok(1000) => {
            let hwnd = editor_hwnd as HWND;

            unsafe {
                ShowWindow(hwnd, SW_SHOW);
                UpdateWindow(hwnd);
            }
        }
        Ok(1001) => {
            if manager_open.load(Relaxed) {
                return Ok(());
            } else {
                let mut input_devices = INPUT_DEVICES.write().unwrap();
                let mut output_devices = OUTPUT_DEVICES.write().unwrap();

                input_devices.clear();
                output_devices.clear();

                if let Ok(devices) = host_clone.input_devices() {
                    for device in devices {
                        if let Ok(name) = device.name() {
                            input_devices.push(name);
                        }
                    }
                }

                if let Ok(devices) = host_clone.output_devices() {
                    for device in devices {
                        if let Ok(name) = device.name() {
                            output_devices.push(name);
                        }
                    }
                }
            }

            let old_devices = CONFIG.devices();

            let window = win::window::build()
                .set_message_callback(|window, message| {
                    device_manager_callback(window, message).unwrap_or_else(|error| {
                        error!("device manager callback failed: {}", error);
                        Some(1)
                    })
                })
                .add_extended_style(win::window::ExtendedStyle::ClientEdge)
                .add_style(win::window::Style::OverlappedWindow)
                .size(480, 320)
                .create(class_clone, "Device Manager")?;

            manager_open.store(true, Relaxed);

            window.show_default();
            _ = window.update();
            win::message_loop();

            manager_open.store(false, Relaxed);

            if old_devices != CONFIG.devices() {
                // restart the backend if the devices have changed
                run_clone.store(false, Relaxed);
            }
        }
        Ok(1002) => {
            Command::new("notepad.exe")
                .arg("whisper_ware.log")
                .spawn()?;
        }
        Ok(1003) => run_clone.store(false, Relaxed),
        Ok(1004) => std::process::exit(0),
        event => error!("Unknown event: {:?}", event),
    }

    Ok(())
}

/// window callback for the device manager
fn device_manager_callback(window: &Window, message: Message) -> Result<Option<isize>> {
    match message {
        Message::Create => {
            let (input_device, output_device) = CONFIG.devices();

            build_device_widget(
                window,
                &INPUT_DEVICES,
                "Input Device",
                &input_device,
                0,
                IDC_INPUT_SELECT,
            )?;

            build_device_widget(
                window,
                &OUTPUT_DEVICES,
                "Output Device",
                &output_device,
                160,
                IDC_OUTPUT_SELECT,
            )?;
        }
        Message::Size(info) => {
            let input_ctrl = window.get_dialog_item(IDC_INPUT_SELECT)?;
            let output_ctrl = window.get_dialog_item(IDC_OUTPUT_SELECT)?;

            input_ctrl.set_rect(
                win::rect::Rect::new(info.width() as i32 / 2, info.height() as i32).at(0, 0),
            )?;

            output_ctrl.set_rect(
                win::rect::Rect::new(info.width() as i32 / 2, info.height() as i32)
                    .at(info.width() as i32 / 2, 0),
            )?;
        }
        Message::Command(info) => unsafe {
            if let Some(control_data) = info.control_data() {
                let mut buffer = [0_u8; 256];
                let hwnd = control_data.window.hwnd_ptr();

                let cur_sel = SendMessageA(hwnd, LB_GETCURSEL, 0, 0);

                // the default value is -1 which will overflow and cause issues
                if cur_sel < 0 {
                    return Ok(None);
                }

                let len = SendMessageA(hwnd, LB_GETTEXTLEN, cur_sel as usize, 0);
                SendMessageA(
                    hwnd,
                    LB_GETTEXT,
                    cur_sel as usize,
                    buffer.as_mut_ptr() as LPARAM,
                );

                let selection = String::from_utf8_lossy(&buffer[..len as usize]).to_string();

                if control_data.id == IDC_INPUT_SELECT {
                    CONFIG.set_input_device(selection)?;
                } else if control_data.id == IDC_OUTPUT_SELECT {
                    CONFIG.set_output_device(selection)?;
                }
            }
        },
        Message::Close => window.destroy()?,
        Message::Destroy => win::post_quit_message(0),
        _ => return Ok(None),
    }

    Ok(Some(0))
}

/// window callback for the configurator
fn editor_callback(window: &Window, message: Message) -> Option<isize> {
    match message {
        Message::Close => {
            // hide the window instead of destroying it
            _ = unsafe { ShowWindow(window.hwnd_ptr(), SW_HIDE) };
            Some(0)
        }
        _ => None,
    }
}

/// builds a list box widget for the device manager
fn build_device_widget(
    window: &Window,
    devices: &RwLock<Vec<String>>,
    name: &str,
    selected: &str,
    x: i32,
    control_id: u16,
) -> Result<()> {
    let ctrl = win::window::build()
        .add_style(win::window::Style::Visible)
        .add_style(win::window::Style::Center)
        .add_style(win::window::Style::Caption)
        .pos(x, 0)
        .size(150, 100)
        .parent(window)
        .set_child_id(control_id)
        .create(win::class::list_box(), name)?;

    let mut selected_output = None;

    for (index, device) in devices.read().unwrap().iter().enumerate() {
        if device == selected {
            selected_output = Some(index);
        }

        // this error is ignored because it is not critical
        _ = ctrl.add_string_item(device);
    }

    if let Some(index) = selected_output {
        let hwnd = ctrl.hwnd_ptr();
        unsafe {
            SendMessageA(hwnd, LB_SETCURSEL, index, 0);
        }
    }

    Ok(())
}

fn device_by_name(device: &Device, other: &str) -> bool {
    if let Ok(name) = device.name() {
        name.contains(other)
    } else {
        false
    }
}
