use std::io::{BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// USB packet information
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct UsbPacket {
    pub timestamp: Duration,
    pub direction: PacketDirection,
    pub endpoint: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketDirection {
    HostToDevice,
    DeviceToHost,
}

/// USB packet monitor using platform-specific tools
/// - Windows: USBPcapCMD subprocess
/// - Linux: usbmon via tcpdump
pub struct UsbMonitor {
    capture_thread: Option<thread::JoinHandle<()>>,
    capture_process: Option<Child>,
    packets: Arc<Mutex<Vec<UsbPacket>>>,
    running: Arc<Mutex<bool>>,
    #[allow(dead_code)]
    device_filter: Option<String>,
}

impl UsbMonitor {
    /// Create a new USB monitor
    pub fn new() -> Self {
        Self {
            capture_thread: None,
            capture_process: None,
            packets: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(Mutex::new(false)),
            device_filter: None,
        }
    }

    /// Set device filter (VID:PID format, e.g., "046D:C24F" for Logitech G29)
    #[allow(dead_code)]
    pub fn set_device_filter(&mut self, filter: String) {
        self.device_filter = Some(filter);
    }

    /// Find USBPcapCMD executable (Windows only)
    #[cfg(target_os = "windows")]
    fn find_usbpcapcmd() -> Option<String> {
        let paths = [
            r"C:\Program Files\USBPcap\USBPcapCMD.exe",
            r"C:\Program Files (x86)\USBPcap\USBPcapCMD.exe",
        ];
        
        for path in &paths {
            if std::path::Path::new(path).exists() {
                return Some(path.to_string());
            }
        }
        
        // Try to find in PATH
        if let Ok(output) = Command::new("where").arg("USBPcapCMD.exe").output() {
            if output.status.success() {
                if let Ok(path) = String::from_utf8(output.stdout) {
                    let path = path.trim().to_string();
                    if !path.is_empty() {
                        return Some(path);
                    }
                }
            }
        }
        
        None
    }

    /// Find USBPcap device number (Windows)
    #[cfg(target_os = "windows")]
    fn find_usbpcap_device() -> Option<u32> {
        // USBPcap creates devices \\.\USBPcap1, \\.\USBPcap2, etc.
        // corresponding to USB root hubs. Try to find one that exists.
        for i in 1..=10 {
            let _device_path = format!(r"\\.\.USBPcap{}", i);
            // Check if the device exists by trying to open it briefly
            // We'll just assume USBPcap1 exists if USBPcap service is running
            // A proper check would require CreateFile API
            if i == 1 {
                return Some(i);
            }
        }
        None
    }

    /// Start capturing USB packets (Windows implementation)
    #[cfg(target_os = "windows")]
    pub fn start_capture(&mut self) -> Result<(), String> {
        // Find USBPcapCMD executable
        let usbpcapcmd = Self::find_usbpcapcmd().ok_or_else(|| {
            "USBPcapCMD.exe not found. Please install USBPcap from https://desowin.org/usbpcap/".to_string()
        })?;

        // Find USBPcap device
        let device_num = Self::find_usbpcap_device().ok_or_else(|| {
            "No USBPcap device found. Please ensure USBPcap is installed and running.".to_string()
        })?;

        let device_path = format!(r"\\.\USBPcap{}", device_num);
        println!("Starting USB packet capture on: {}", device_path);
        println!("Using: {}", usbpcapcmd);
        println!("NOTE: USB capture requires Administrator privileges");

        // Start USBPcapCMD with output to stdout (pipe)
        // Using "-" as output means stdout
        // Use CREATE_NO_WINDOW to prevent console popups
        #[cfg(target_os = "windows")]
        use std::os::windows::process::CommandExt;
        
        let mut command = Command::new(&usbpcapcmd);
        command
            .args([
                "-d", &device_path,
                "-o", "-",  // Output to stdout
                "-A",       // Capture from all devices on this hub
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null());
        
        // Hide the console window for the subprocess
        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        
        let mut child = command
            .spawn()
            .map_err(|e| format!("Failed to start USBPcapCMD: {}", e))?;

        let stdout = child.stdout.take().ok_or("Failed to get stdout from USBPcapCMD")?;
        
        let packets = Arc::clone(&self.packets);
        let running = Arc::clone(&self.running);
        
        *running.lock().unwrap() = true;
        
        self.capture_process = Some(child);
        
        self.capture_thread = Some(thread::spawn(move || {
            Self::pcap_reader_loop(stdout, packets, running);
        }));

        thread::sleep(Duration::from_millis(5000)); // Give some time to start capturing

        Ok(())
    }

    /// Find usbmon interface (Linux)
    #[cfg(target_os = "linux")]
    fn find_usbmon_interface() -> Option<String> {
        // Check if usbmon module is loaded
        if std::path::Path::new("/sys/module/usbmon").exists() {
            // usbmon0 captures all buses, usbmon1, usbmon2, etc. for specific buses
            // Try usbmon0 first (captures all)
            if std::path::Path::new("/dev/usbmon0").exists() {
                return Some("usbmon0".to_string());
            }
            // Fallback to bus-specific interfaces
            for i in 1..=10 {
                let path = format!("/dev/usbmon{}", i);
                if std::path::Path::new(&path).exists() {
                    return Some(format!("usbmon{}", i));
                }
            }
        }
        // Even without /dev/usbmon*, tcpdump can use usbmon interfaces
        Some("usbmon0".to_string())
    }

    /// Start capturing USB packets (Linux implementation)
    #[cfg(target_os = "linux")]
    pub fn start_capture(&mut self) -> Result<(), String> {
        // Check for tcpdump
        if Command::new("which").arg("tcpdump").output().map(|o| !o.status.success()).unwrap_or(true) {
            return Err("tcpdump not found. Please install tcpdump: sudo apt install tcpdump".to_string());
        }

        // Load usbmon module if not loaded
        let _ = Command::new("sudo")
            .args(["modprobe", "usbmon"])
            .output();

        let interface = Self::find_usbmon_interface().ok_or_else(|| {
            "No usbmon interface found. Please ensure usbmon kernel module is loaded: sudo modprobe usbmon".to_string()
        })?;

        println!("Starting USB packet capture on: {}", interface);
        println!("Using: tcpdump (may require sudo/root)");

        // Start tcpdump to capture USB packets in pcap format
        // -i: interface, -w -: write to stdout, -U: unbuffered
        let mut child = Command::new("sudo")
            .args([
                "tcpdump",
                "-i", &interface,
                "-w", "-",  // Output to stdout in pcap format
                "-U",       // Unbuffered output
                "-q",       // Quiet mode
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start tcpdump: {}. Try running with sudo.", e))?;

        let stdout = child.stdout.take().ok_or("Failed to get stdout from tcpdump")?;
        
        let packets = Arc::clone(&self.packets);
        let running = Arc::clone(&self.running);
        
        *running.lock().unwrap() = true;
        
        self.capture_process = Some(child);
        
        self.capture_thread = Some(thread::spawn(move || {
            Self::linux_pcap_reader_loop(stdout, packets, running);
        }));

        thread::sleep(Duration::from_millis(2000)); // Give some time to start capturing

        Ok(())
    }

    /// Read pcap data from stdout (Windows - USBPcap format)
    #[cfg(target_os = "windows")]
    fn pcap_reader_loop<R: Read>(
        stdout: R,
        packets: Arc<Mutex<Vec<UsbPacket>>>,
        running: Arc<Mutex<bool>>,
    ) {
        let mut reader = BufReader::new(stdout);
        let mut buffer = vec![0u8; 65536];
        let mut pcap_buffer = Vec::new();
        let mut header_read = false;
        
        loop {
            // Check running flag without holding lock during read
            if !*running.lock().unwrap() {
                break;
            }
            
            match reader.read(&mut buffer) {
                Ok(0) => {
                    // EOF - process exited (could be permission error)
                    break;
                }
                Ok(n) => {
                    pcap_buffer.extend_from_slice(&buffer[..n]);
                    
                    // Skip pcap global header (24 bytes) on first read
                    if !header_read && pcap_buffer.len() >= 24 {
                        // Verify pcap magic
                        if pcap_buffer[0..4] == [0xd4, 0xc3, 0xb2, 0xa1] || 
                           pcap_buffer[0..4] == [0xa1, 0xb2, 0xc3, 0xd4] {
                            pcap_buffer = pcap_buffer[24..].to_vec();
                            header_read = true;
                        } else {
                            // Invalid pcap header - could be error message from USBPcapCMD
                            // Check if it looks like an error message
                            if let Ok(text) = String::from_utf8(pcap_buffer[..n.min(100)].to_vec()) {
                                if text.contains("Couldn't open") || text.contains("Access") {
                                    eprintln!("ERROR: USB capture failed. Run as Administrator.");
                                }
                            }
                            break;
                        }
                    }
                    
                    // Parse pcap packets from buffer
                    while pcap_buffer.len() >= 16 {
                        // pcap packet header: ts_sec(4), ts_usec(4), incl_len(4), orig_len(4)
                        let incl_len = u32::from_le_bytes([
                            pcap_buffer[8], pcap_buffer[9], 
                            pcap_buffer[10], pcap_buffer[11]
                        ]) as usize;
                        
                        let total_packet_len = 16 + incl_len;
                        
                        if pcap_buffer.len() < total_packet_len {
                            // Need more data
                            break;
                        }
                        
                        // Extract packet data (skip pcap packet header)
                        let packet_data = &pcap_buffer[16..total_packet_len];
                        
                        // Parse USB packet
                        if let Some(usb_packet) = Self::parse_usbpcap_packet(packet_data) {
                            packets.lock().unwrap().push(usb_packet);
                        }
                        
                        // Remove processed packet from buffer
                        pcap_buffer = pcap_buffer[total_packet_len..].to_vec();
                    }
                }
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::WouldBlock {
                        break;
                    }
                }
            }
        }
    }

    /// Read pcap data from stdout (Linux - usbmon format)
    #[cfg(target_os = "linux")]
    fn linux_pcap_reader_loop<R: Read>(
        stdout: R,
        packets: Arc<Mutex<Vec<UsbPacket>>>,
        running: Arc<Mutex<bool>>,
    ) {
        let mut reader = BufReader::new(stdout);
        let mut buffer = vec![0u8; 65536];
        let mut pcap_buffer = Vec::new();
        let mut header_read = false;
        
        println!("USB capture started (reading from tcpdump/usbmon)");
        
        while *running.lock().unwrap() {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    // EOF - process exited
                    break;
                }
                Ok(n) => {
                    pcap_buffer.extend_from_slice(&buffer[..n]);
                    
                    // Skip pcap global header (24 bytes) on first read
                    if !header_read && pcap_buffer.len() >= 24 {
                        // Verify pcap magic (both endianness)
                        if pcap_buffer[0..4] == [0xd4, 0xc3, 0xb2, 0xa1] || 
                           pcap_buffer[0..4] == [0xa1, 0xb2, 0xc3, 0xd4] {
                            pcap_buffer = pcap_buffer[24..].to_vec();
                            header_read = true;
                        } else {
                            eprintln!("WARNING: Invalid pcap header: {:02X?}", &pcap_buffer[0..4]);
                            break;
                        }
                    }
                    
                    // Parse pcap packets from buffer
                    while pcap_buffer.len() >= 16 {
                        // pcap packet header: ts_sec(4), ts_usec(4), incl_len(4), orig_len(4)
                        let incl_len = u32::from_le_bytes([
                            pcap_buffer[8], pcap_buffer[9], 
                            pcap_buffer[10], pcap_buffer[11]
                        ]) as usize;
                        
                        let total_packet_len = 16 + incl_len;
                        
                        if pcap_buffer.len() < total_packet_len {
                            // Need more data
                            break;
                        }
                        
                        // Extract packet data (skip pcap packet header)
                        let packet_data = &pcap_buffer[16..total_packet_len];
                        
                        // Parse usbmon packet
                        if let Some(usb_packet) = Self::parse_usbmon_packet(packet_data) {
                            packets.lock().unwrap().push(usb_packet);
                        }
                        
                        // Remove processed packet from buffer
                        pcap_buffer = pcap_buffer[total_packet_len..].to_vec();
                    }
                }
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::WouldBlock {
                        eprintln!("WARNING: Read error: {}", e);
                        break;
                    }
                }
            }
        }
    }

    /// Parse USBPcap packet (Windows)
    #[cfg(target_os = "windows")]
    fn parse_usbpcap_packet(data: &[u8]) -> Option<UsbPacket> {
        // USBPcap header format:
        // Offset 0: headerLen (2 bytes, LE) - usually 27 or 28
        // Offset 2: irpId (8 bytes)
        // Offset 10: usbd_status (4 bytes)
        // Offset 14: function (2 bytes)
        // Offset 16: info (1 byte) - direction bit at 0x01
        // Offset 17: bus (2 bytes)
        // Offset 19: device (2 bytes)
        // Offset 21: endpoint (1 byte) - endpoint with direction
        // Offset 22: transfer (1 byte) - transfer type
        // Offset 23: dataLength (4 bytes)
        // After header: payload data
        
        if data.len() < 27 {
            return None;
        }

        let header_len = u16::from_le_bytes([data[0], data[1]]) as usize;
        if data.len() < header_len {
            return None;
        }

        // Extract info byte (direction)
        let info = data[16];
        let direction = if (info & 0x01) != 0 {
            PacketDirection::DeviceToHost // PDO -> FDO (IN)
        } else {
            PacketDirection::HostToDevice // FDO -> PDO (OUT)
        };

        // Extract endpoint
        let endpoint = data[21] & 0x7F;

        // Extract transfer type
        let transfer_type = data[22];
        
        // We're interested in Interrupt and Control transfers for FFB
        // Transfer types: 0=Isochronous, 1=Interrupt, 2=Control, 3=Bulk
        if transfer_type != 1 && transfer_type != 2 {
            return None;
        }

        // Extract payload data
        let payload_data = if data.len() > header_len {
            data[header_len..].to_vec()
        } else {
            Vec::new()
        };

        // Filter out empty packets
        if payload_data.is_empty() {
            return None;
        }

        // Filter for FFB-related packets only:
        // 1. Only Host-to-Device (OUT) direction - these are FFB commands
        // 2. Filter out device-to-host (IN) packets which are typically input reports
        if direction == PacketDirection::DeviceToHost {
            return None;
        }

        // Filter for likely FFB commands based on common patterns:
        // - HID SET_REPORT for FFB typically has specific report IDs
        // - Logitech wheels use report IDs like 0x11, 0x13, etc.
        // - Many FFB devices use endpoint 0 (control) or specific interrupt endpoints
        
        // For control transfers (type 2), look for SET_REPORT requests
        // For interrupt transfers (type 1), accept OUT packets to common FFB endpoints
        
        // Accept packets that look like FFB commands:
        // - Minimum size for meaningful FFB data
        if payload_data.len() < 2 {
            return None;
        }

        Some(UsbPacket {
            timestamp: Duration::from_micros(0), // Could extract from packet if needed
            direction,
            endpoint,
            data: payload_data,
        })
    }

    /// Parse usbmon packet (Linux)
    /// usbmon binary format (64 bytes header for USB packets):
    /// See: https://www.kernel.org/doc/Documentation/usb/usbmon.txt
    #[cfg(target_os = "linux")]
    fn parse_usbmon_packet(data: &[u8]) -> Option<UsbPacket> {
        // usbmon header (mon_bin_hdr) is 64 bytes:
        // Offset 0:  id (8 bytes) - URB id
        // Offset 8:  type (1 byte) - 'S'ubmit, 'C'omplete, 'E'rror
        // Offset 9:  xfer_type (1 byte) - 0=ISO, 1=Interrupt, 2=Control, 3=Bulk
        // Offset 10: epnum (1 byte) - endpoint with direction (bit 7 = direction)
        // Offset 11: devnum (1 byte) - device number
        // Offset 12: busnum (2 bytes) - bus number
        // Offset 14: flag_setup (1 byte) - 0 if setup present
        // Offset 15: flag_data (1 byte) - 0 if data present
        // Offset 16: ts_sec (8 bytes) - timestamp seconds
        // Offset 24: ts_usec (4 bytes) - timestamp microseconds
        // Offset 28: status (4 bytes)
        // Offset 32: length (4 bytes) - data length
        // Offset 36: len_cap (4 bytes) - captured length
        // Offset 40: setup (8 bytes) - setup packet if control transfer
        // Offset 48: interval (4 bytes)
        // Offset 52: start_frame (4 bytes)
        // Offset 56: xfer_flags (4 bytes)
        // Offset 60: ndesc (4 bytes)
        // After header: payload data
        
        const USBMON_HEADER_LEN: usize = 64;
        
        if data.len() < USBMON_HEADER_LEN {
            return None;
        }

        let event_type = data[8] as char;
        let xfer_type = data[9];
        let epnum = data[10];
        
        // Direction: bit 7 of epnum (0 = OUT, 1 = IN)
        let direction = if (epnum & 0x80) != 0 {
            PacketDirection::DeviceToHost
        } else {
            PacketDirection::HostToDevice
        };
        let endpoint = epnum & 0x7F;

        // We're interested in Submit ('S') events for OUT, Complete ('C') for IN
        // For FFB monitoring, we want OUT packets (host to device)
        if direction == PacketDirection::DeviceToHost {
            return None;
        }
        
        // Only process Submit events for OUT transfers
        if event_type != 'S' {
            return None;
        }

        // Filter for Interrupt (1) and Control (2) transfers
        if xfer_type != 1 && xfer_type != 2 {
            return None;
        }

        // Extract captured length
        let len_cap = u32::from_le_bytes([data[36], data[37], data[38], data[39]]) as usize;
        
        // Extract payload data
        let payload_data = if data.len() > USBMON_HEADER_LEN && len_cap > 0 {
            let payload_end = std::cmp::min(USBMON_HEADER_LEN + len_cap, data.len());
            data[USBMON_HEADER_LEN..payload_end].to_vec()
        } else {
            Vec::new()
        };

        // Filter out empty packets
        if payload_data.is_empty() || payload_data.len() < 2 {
            return None;
        }

        // Extract timestamp
        let ts_sec = u64::from_le_bytes([
            data[16], data[17], data[18], data[19],
            data[20], data[21], data[22], data[23],
        ]);
        let ts_usec = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        let timestamp = Duration::from_secs(ts_sec) + Duration::from_micros(ts_usec as u64);

        Some(UsbPacket {
            timestamp,
            direction,
            endpoint,
            data: payload_data,
        })
    }

    /// Check if packet looks like an FFB command
    pub fn is_ffb_command(packet: &UsbPacket) -> bool {
        // FFB commands are always Host-to-Device
        if packet.direction != PacketDirection::HostToDevice {
            return false;
        }
        
        // Must have some data
        if packet.data.is_empty() {
            return false;
        }

        // Common FFB report IDs and patterns:
        // Logitech: 0x11, 0x13, 0x14, 0xF3, 0xF5
        // Generic HID FFB: first byte is often report ID
        let first_byte = packet.data[0];
        
        // Accept common FFB report IDs
        matches!(first_byte, 
            0x11 | 0x12 | 0x13 | 0x14 | 0x15 |  // Logitech FFB commands
            0xF3 | 0xF5 |                         // Logitech extended commands
            0x01..=0x0F |                         // Generic HID FFB report IDs
            0x21                                  // SET_REPORT request type
        ) || packet.data.len() >= 7  // Or any substantial OUT packet
    }

    /// Get and clear captured packets
    pub fn get_packets(&self) -> Vec<UsbPacket> {
        let mut packets = self.packets.lock().unwrap();
        let result = packets.clone();
        packets.clear();
        result
    }

    /// Stop capturing
    pub fn stop_capture(&mut self) {
        // Set running to false first to stop the reader loop
        *self.running.lock().unwrap() = false;
        
        // Kill the capture process (USBPcapCMD on Windows, tcpdump on Linux)
        // This will cause "Write failed" message from USBPcapCMD which is expected
        if let Some(mut child) = self.capture_process.take() {
            // On Windows, terminate more gracefully
            #[cfg(target_os = "windows")]
            {
                // Try to kill gracefully first
                let _ = child.kill();
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = child.kill();
            }
            // Wait for process to exit
            let _ = child.wait();
        }
        
        if let Some(thread) = self.capture_thread.take() {
            let _ = thread.join();
        }
    }

    /// Print packet in hex format
    #[allow(dead_code)]
    pub fn print_packet(packet: &UsbPacket, prefix: &str) {
        let direction_str = match packet.direction {
            PacketDirection::HostToDevice => "→ OUT",
            PacketDirection::DeviceToHost => "← IN ",
        };
        
        println!("{}{} EP{:02X} ({} bytes):", prefix, direction_str, packet.endpoint, packet.data.len());
        
        if !packet.data.is_empty() {
            print!("{}  ", prefix);
            for (i, byte) in packet.data.iter().enumerate() {
                print!("{:02X} ", byte);
                if (i + 1) % 16 == 0 && i + 1 < packet.data.len() {
                    print!("\n{}  ", prefix);
                }
            }
            println!();
        }
    }
}

impl Drop for UsbMonitor {
    fn drop(&mut self) {
        self.stop_capture();
    }
}

/// Helper function to format packet data as hex string
pub fn format_hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ")
}
