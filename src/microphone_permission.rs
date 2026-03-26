use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub fn request_microphone_access(preferred_device_name: Option<&str>) -> Result<(), String> {
    let host = cpal::default_host();
    let device = get_input_device(&host, preferred_device_name)?;

    let supported = device
        .default_input_config()
        .map_err(|err| permission_error_message(format!("Failed to get input config: {err}")))?;

    let stream_config = supported.config();

    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device
            .build_input_stream(&stream_config, |_data: &[f32], _| {}, stream_err, None)
            .map_err(|err| permission_error_message(format!("Failed to open input stream: {err}")))?,
        cpal::SampleFormat::I16 => device
            .build_input_stream(&stream_config, |_data: &[i16], _| {}, stream_err, None)
            .map_err(|err| permission_error_message(format!("Failed to open input stream: {err}")))?,
        cpal::SampleFormat::U16 => device
            .build_input_stream(&stream_config, |_data: &[u16], _| {}, stream_err, None)
            .map_err(|err| permission_error_message(format!("Failed to open input stream: {err}")))?,
        other => {
            return Err(permission_error_message(format!(
                "Unsupported input sample format: {other:?}"
            )))
        }
    };

    stream
        .play()
        .map_err(|err| permission_error_message(format!("Failed to start input stream: {err}")))?;

    std::thread::sleep(std::time::Duration::from_millis(120));
    Ok(())
}

fn get_input_device(
    host: &cpal::Host,
    preferred_device_name: Option<&str>,
) -> Result<cpal::Device, String> {
    if let Some(name) = preferred_device_name {
        let found = host.input_devices().ok().and_then(|devices| {
            devices.filter_map(|d| d.name().ok().map(|n| (d, n))).find_map(
                |(device, device_name)| {
                    if device_name == name {
                        Some(device)
                    } else {
                        None
                    }
                },
            )
        });

        if let Some(device) = found {
            return Ok(device);
        }
    }

    host.default_input_device()
        .ok_or_else(|| permission_error_message("No input device available".to_string()))
}

fn stream_err(err: cpal::StreamError) {
    eprintln!("Microphone permission probe stream error: {err}");
}

fn permission_error_message(reason: String) -> String {
    format!(
        "Microphone access is unavailable. Grant microphone permission for wgo in your OS privacy settings and try again. Details: {reason}"
    )
}