use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub source: String,
    pub port: Option<String>,
    pub display_name: String,
    pub short_name: String,
}

impl AudioDevice {
    pub fn discover() -> (Vec<AudioDevice>, usize) {
        let output = match Command::new("pactl").args(["list", "sources"]).output() {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
            _ => return (Vec::new(), 0),
        };

        let default_source = match Command::new("pactl").args(["get-default-source"]).output() {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            _ => String::new(),
        };

        let mut devices = Vec::new();
        let mut active_idx = 0;

        let mut cur_name = String::new();
        let mut cur_desc = String::new();
        let mut cur_active_port = String::new();
        let mut cur_ports: Vec<(String, String)> = Vec::new();
        let mut in_ports = false;

        let flush_source = |name: &str,
                                desc: &str,
                                active_port: &str,
                                ports: &[(String, String)],
                                devs: &mut Vec<AudioDevice>,
                                cur_active_idx: &mut usize| {
            if name.is_empty()
                || name.ends_with(".monitor")
                || name.starts_with("wstui-")
                || name.contains("echo")
            {
                return;
            }

            let is_default_source = name == default_source;

            if ports.is_empty() {
                let (disp, short) = format_source_names(name, desc);
                let idx = devs.len();
                devs.push(AudioDevice {
                    source: name.to_string(),
                    port: None,
                    display_name: disp,
                    short_name: short,
                });
                if is_default_source {
                    *cur_active_idx = idx;
                }
            } else {
                for (p_id, p_desc) in ports {
                    let (disp, short) = format_port_names(p_id, p_desc, desc);
                    let idx = devs.len();
                    devs.push(AudioDevice {
                        source: name.to_string(),
                        port: Some(p_id.clone()),
                        display_name: disp,
                        short_name: short,
                    });
                    if is_default_source && p_id == active_port {
                        *cur_active_idx = idx;
                    }
                }
            }
        };

        for line in output.lines() {
            if line.starts_with("Source #") {
                flush_source(
                    &cur_name,
                    &cur_desc,
                    &cur_active_port,
                    &cur_ports,
                    &mut devices,
                    &mut active_idx,
                );
                cur_name.clear();
                cur_desc.clear();
                cur_active_port.clear();
                cur_ports.clear();
                in_ports = false;
                continue;
            }

            if line.starts_with("\tName: ") {
                cur_name = line.trim_start_matches("\tName: ").trim().to_string();
            } else if line.starts_with("\tDescription: ") {
                cur_desc = line.trim_start_matches("\tDescription: ").trim().to_string();
            } else if line.starts_with("\tActive Port: ") {
                cur_active_port = line.trim_start_matches("\tActive Port: ").trim().to_string();
                in_ports = false;
            } else if line.starts_with("\tPorts:") {
                in_ports = true;
            } else if in_ports && line.starts_with("\t\t") {
                let trimmed = line.trim();
                if let Some((p_id, rest)) = trimmed.split_once(':') {
                    let p_id = p_id.trim();
                    let rest = rest.trim();
                    let p_desc = if let Some(idx) = rest.find('(') {
                        rest[..idx].trim()
                    } else {
                        rest
                    };
                    cur_ports.push((p_id.to_string(), p_desc.to_string()));
                }
            } else if in_ports && !line.starts_with("\t\t") {
                in_ports = false;
            }
        }

        flush_source(
            &cur_name,
            &cur_desc,
            &cur_active_port,
            &cur_ports,
            &mut devices,
            &mut active_idx,
        );

        (devices, active_idx)
    }

    pub fn apply(&self, is_call_active: bool) {
        log::info!(
            "Aplicando micrófono: {} (source: {}, port: {:?})",
            self.display_name,
            self.source,
            self.port
        );

        // 1. Set default source in PulseAudio/PipeWire
        let _ = Command::new("pactl")
            .args(["set-default-source", &self.source])
            .output();

        // 2. Set source port if available
        if let Some(port) = &self.port {
            let _ = Command::new("pactl")
                .args(["set-source-port", &self.source, port])
                .output();
        }

        // 3. If a call is currently active, re-route running streams
        if is_call_active {
            reroute_active_streams(&self.source);
        }
    }
}

fn reroute_active_streams(target_source: &str) {
    if let Ok(output) = Command::new("pactl").args(["list", "source-outputs"]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        for block in text.split("Source Output #") {
            if block.is_empty() {
                continue;
            }
            let first_line = block.lines().next().unwrap_or("");
            let out_id = first_line.trim();
            if out_id.is_empty() || !out_id.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }

            let is_echo_cancel_capture = block.contains("node.name = \"echo-cancel-capture\"");
            let is_parec = block.contains("application.name = \"parec\"");
            let is_on_echo_source = block.contains("target.object = \"wstui-echo-source\"")
                || block.contains("wstui-echo-source");

            // If echo-cancel is active, move echo-cancel-capture to the new hardware source.
            // If parec is recording directly (no echo-cancel), move parec to the new source.
            if is_echo_cancel_capture || (is_parec && !is_on_echo_source) {
                log::info!("Moviendo stream de audio #{} hacia {}", out_id, target_source);
                let _ = Command::new("pactl")
                    .args(["move-source-output", out_id, target_source])
                    .output();
            }
        }
    }
}

fn format_port_names(port_id: &str, port_desc: &str, card_desc: &str) -> (String, String) {
    let lower_id = port_id.to_lowercase();
    let lower_desc = port_desc.to_lowercase();
    if lower_id.contains("internal") || lower_desc.contains("interno") || lower_desc.contains("internal") {
        ("Micrófono Interno".to_string(), "Mic Interno".to_string())
    } else if lower_id.contains("headset") || lower_desc.contains("headset") || lower_desc.contains("auricular") {
        ("Micrófono Auricular / Jack".to_string(), "Mic Jack".to_string())
    } else if lower_id == "analog-input-mic" || lower_desc == "micrófono" || lower_desc == "microphone" {
        ("Micrófono Auricular / Jack".to_string(), "Mic Jack".to_string())
    } else {
        let display = if !card_desc.is_empty() && card_desc != "(null)" {
            format!("{port_desc} ({card_desc})")
        } else {
            port_desc.to_string()
        };
        let short = if port_desc.chars().count() > 14 {
            let s: String = port_desc.chars().take(12).collect();
            format!("{}...", s)
        } else {
            port_desc.to_string()
        };
        (display, short)
    }
}

fn format_source_names(name: &str, desc: &str) -> (String, String) {
    let display = if !desc.is_empty() && desc != "(null)" {
        desc.to_string()
    } else {
        name.to_string()
    };
    let lower = display.to_lowercase();
    let short = if lower.contains("usb") {
        "Mic USB".to_string()
    } else if lower.contains("bluetooth") || lower.contains("bluez") {
        "Mic BT".to_string()
    } else if display.chars().count() > 14 {
        let s: String = display.chars().take(12).collect();
        format!("{}...", s)
    } else {
        display.clone()
    };
    (display, short)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover() {
        let (devs, active) = AudioDevice::discover();
        println!("Discovered {} devices, active index: {}", devs.len(), active);
        for (i, d) in devs.iter().enumerate() {
            println!("[{}] {:?} (active: {})", i, d, i == active);
        }
        assert!(!devs.is_empty());
    }
}
