# WhisperWare
I developed this software to enhance quiet sounds in competitive games while maintaining a safe overall volume level by compressing and amplifying audio in real time
## Setup
1. Download and install a Virtual Audio Cable. I recommend the Lite version of this [VAC](https://vac.muzychenko.net/en/download.htm) as it is free and seems to have reliably good audio quality
2. In your Windows sound settings, ensure that the input and output of your VAC have the same configuration as your output device. I recommend selecting 16-bit at 44100 Hz; stereo is required
3. Download and install Whisper Ware from the [releases](https://github.com/chanderlud/whisper-ware/releases). Using the installer version is recommended; if you choose to manually install, you will need to download [Rough Rider 3](https://www.audiodamage.com/pages/free-and-legacy) and place the VST plugin DLL in the same directory as Whisper Ware
4. Launch WhisperWare, select the device manager from the tray application, set the input device to your VAC, and the output device to your normal output device
5. In your game, select your VAC as the output device. Configure Whisper Ware options from the configurator
## Troubleshooting
- Checking the logs via the tray application can help diagnose issues
- Try restarting the backend from the tray application
- Try a different audio source application to see if the issue is with the game
