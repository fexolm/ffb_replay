mod driver;
mod drivers;
mod effects;
mod error;
mod usb_monitor;

use clap::{Parser, Subcommand};
use driver::FfbDriver;
use drivers::sdl_driver::SdlDriver;
use drivers::simagic_driver::SimagicDriver;
use effects::Effect;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Scenario step - effect with delay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioStep {
    /// Effect
    pub effect: Effect,
}

/// Playback scenario
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    /// Scenario name
    pub name: String,
    /// Description
    #[serde(default)]
    pub description: String,
    /// Loop forever
    #[serde(default)]
    pub loop_forever: bool,
    /// Repeat count (if not loop_forever)
    #[serde(default = "default_repeat_count")]
    pub repeat_count: u32,
    /// Scenario steps
    pub steps: Vec<ScenarioStep>,
}

fn default_repeat_count() -> u32 {
    1
}

/// Captured output for a single step
#[derive(Debug, Clone)]
pub struct StepOutput {
    pub step_index: usize,
    pub step_name: String,
    pub packets: Vec<String>,
}

impl Scenario {
    /// Load scenario from YAML file
    pub fn load_from_file(path: &PathBuf) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path)?;
        let scenario: Scenario = serde_yaml::from_str(&content)?;
        Ok(scenario)
    }

    /// Play scenario with a specific driver
    /// Returns captured/generated packets organized by step
    pub fn play<D: FfbDriver + ?Sized>(&self, driver: &mut D) -> anyhow::Result<Vec<StepOutput>> {
        println!("Starting scenario: {}", self.name);
        if !self.description.is_empty() {
            println!("  {}", self.description);
        }
        println!();

        let iterations = if self.loop_forever {
            println!("WARNING: Infinite loop mode. Press Ctrl+C to stop.");
            u32::MAX
        } else {
            self.repeat_count
        };

        let mut all_outputs: Vec<StepOutput> = Vec::new();

        for iteration in 0..iterations {
            if iterations != u32::MAX {
                println!("=== Iteration {}/{} ===", iteration + 1, iterations);
            }

            for (idx, step) in self.steps.iter().enumerate() {
                let effect_type = match &step.effect {
                    Effect::Constant { .. } => "Constant force",
                    Effect::Periodic { effect, .. } => match effect.wave_type {
                        effects::WaveType::Sine => "Periodic (sine)",
                        effects::WaveType::Square => "Periodic (square)",
                        effects::WaveType::Triangle => "Periodic (triangle)",
                        effects::WaveType::SawtoothUp => "Periodic (sawtooth up)",
                        effects::WaveType::SawtoothDown => "Periodic (sawtooth down)",
                    },
                    Effect::Ramp { .. } => "Ramp (linear change)",
                    Effect::Condition { effect, .. } => match effect.condition_type {
                        effects::ConditionType::Spring => "Condition (spring)",
                        effects::ConditionType::Damper => "Condition (damper)",
                        effects::ConditionType::Friction => "Condition (friction)",
                        effects::ConditionType::Inertia => "Condition (inertia)",
                    },
                };

                println!(
                    "  Step {}: {} (duration: {} ms)",
                    idx + 1,
                    effect_type,
                    step.effect.duration()
                );

                // apply_effect returns captured packets and handles timing internally
                // Don't crash on error - just print warning and return empty result
                let packets = match driver.apply_effect(&step.effect) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("    ERROR: Failed to execute effect: {}", e);
                        Vec::new()
                    }
                };

                // Print captured output
                if !packets.is_empty() {
                    println!("    Output ({} packets):", packets.len());
                    for packet in &packets {
                        println!("      {}", packet);
                    }
                } else {
                    println!("    Output: (no packets captured)");
                }

                all_outputs.push(StepOutput {
                    step_index: idx + 1,
                    step_name: effect_type.to_string(),
                    packets,
                });

                let _ = driver.stop_all_effects();
            }

            println!();
        }

        println!("Scenario completed");
        Ok(all_outputs)
    }
}

#[derive(Parser)]
#[command(name = "ffb_replay")]
#[command(about = "Force Feedback Replay Tool - Play and compare FFB scenarios", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Play a scenario and capture driver output to a file
    Record {
        /// Path to scenario YAML file
        #[arg(short, long)]
        scenario: PathBuf,

        /// Output file name (will be saved in runs/)
        #[arg(short, long)]
        output: String,

        /// Driver to use: sdl or simagic
        #[arg(short, long, default_value = "sdl")]
        driver: String,
    },
    /// Play a scenario and compare driver output with a capture file
    Compare {
        /// Path to scenario YAML file
        #[arg(short, long)]
        scenario: PathBuf,

        /// Capture file name to compare with (in runs/)
        #[arg(short, long)]
        compare: String,

        /// Driver to use: sdl or simagic
        #[arg(short, long, default_value = "sdl")]
        driver: String,
    },
}

fn create_driver(driver_name: &str) -> anyhow::Result<Box<dyn FfbDriver>> {
    match driver_name.to_lowercase().as_str() {
        "sdl" => Ok(Box::new(SdlDriver::new())),
        "simagic" => Ok(Box::new(SimagicDriver::new())),
        _ => Err(anyhow::anyhow!(
            "Unknown driver: {}. Available drivers: sdl, simagic",
            driver_name
        )),
    }
}

/// Parse a capture file with step markers into StepOutput list
fn parse_capture_file(path: &PathBuf) -> anyhow::Result<Vec<StepOutput>> {
    let content = fs::read_to_string(path)?;
    let mut steps: Vec<StepOutput> = Vec::new();
    let mut current_step: Option<StepOutput> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with("# Step ") {
            // Save previous step if any
            if let Some(step) = current_step.take() {
                steps.push(step);
            }

            // Parse step header: "# Step N: Name"
            let rest = &line[7..]; // Skip "# Step "
            if let Some(colon_pos) = rest.find(':') {
                let step_index = rest[..colon_pos].trim().parse::<usize>().unwrap_or(steps.len() + 1);
                let step_name = rest[colon_pos + 1..].trim().to_string();
                current_step = Some(StepOutput {
                    step_index,
                    step_name,
                    packets: Vec::new(),
                });
            }
        } else if !line.starts_with('#') {
            // Packet data
            if let Some(ref mut step) = current_step {
                step.packets.push(line.to_string());
            } else {
                // No step header yet - create implicit step 1
                current_step = Some(StepOutput {
                    step_index: 1,
                    step_name: "Unknown".to_string(),
                    packets: vec![line.to_string()],
                });
            }
        }
    }

    // Don't forget the last step
    if let Some(step) = current_step {
        steps.push(step);
    }

    Ok(steps)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Record {
            scenario,
            output,
            driver,
        } => {
            if !scenario.exists() {
                eprintln!("Error: Scenario file not found: {}", scenario.display());
                std::process::exit(1);
            }

            println!("Loading scenario: {}", scenario.display());
            let scenario_data = Scenario::load_from_file(&scenario)?;

            // Create runs directory if it doesn't exist
            fs::create_dir_all("runs")?;
            let output_path = PathBuf::from("runs").join(&output);

            println!("Initializing {} driver...", driver);
            let mut driver_instance = create_driver(&driver)?;
            driver_instance.initialize()?;
            println!("Driver ready\n");

            // Play scenario and collect captured packets
            let step_outputs = scenario_data.play(driver_instance.as_mut())?;

            // Save captured packets to file with step markers
            let mut file = fs::File::create(&output_path)?;
            use std::io::Write;
            let mut total_packets = 0;
            for step_output in &step_outputs {
                writeln!(file, "# Step {}: {}", step_output.step_index, step_output.step_name)?;
                for packet in &step_output.packets {
                    writeln!(file, "{}", packet)?;
                }
                total_packets += step_output.packets.len();
            }

            println!("\nSaved {} packets ({} steps) to {}", total_packets, step_outputs.len(), output_path.display());

            println!("\nStopping driver...");
            driver_instance.shutdown()?;
            println!("Done");
        }

        Commands::Compare {
            scenario,
            compare,
            driver,
        } => {
            if !scenario.exists() {
                eprintln!("Error: Scenario file not found: {}", scenario.display());
                std::process::exit(1);
            }

            let compare_path = PathBuf::from("runs").join(&compare);
            if !compare_path.exists() {
                eprintln!("Error: Comparison file not found: {}", compare_path.display());
                std::process::exit(1);
            }

            println!("Loading scenario: {}", scenario.display());
            let scenario_data = Scenario::load_from_file(&scenario)?;

            println!("Loading comparison data: {}", compare_path.display());
            let expected_steps = parse_capture_file(&compare_path)?;

            println!("Initializing {} driver...", driver);
            let mut driver_instance = create_driver(&driver)?;
            driver_instance.initialize()?;
            println!("Driver ready\n");

            // Play scenario and collect captured packets
            let actual_steps = scenario_data.play(driver_instance.as_mut())?;

            // Compare step by step
            println!("\n=== Comparison Results ===");
            println!("Expected: {} steps", expected_steps.len());
            println!("Actual: {} steps\n", actual_steps.len());

            let max_steps = expected_steps.len().max(actual_steps.len());
            let mut mismatched_steps = 0;

            for step_idx in 0..max_steps {
                let expected = expected_steps.get(step_idx);
                let actual = actual_steps.get(step_idx);

                match (expected, actual) {
                    (Some(exp), Some(act)) => {
                        // Compare packets in this step
                        let packets_match = exp.packets == act.packets;
                        
                        if !packets_match {
                            mismatched_steps += 1;
                            println!("MISMATCH Step {}: {}", act.step_index, act.step_name);
                            println!("  Expected {} packets, got {} packets", exp.packets.len(), act.packets.len());
                            
                            // Show differing packets
                            let max_packets = exp.packets.len().max(act.packets.len());
                            for i in 0..max_packets {
                                let exp_pkt = exp.packets.get(i);
                                let act_pkt = act.packets.get(i);
                                
                                match (exp_pkt, act_pkt) {
                                    (Some(e), Some(a)) if e != a => {
                                        println!("    Packet {} differs:", i + 1);
                                        println!("      Expected: {}", e);
                                        println!("      Actual:   {}", a);
                                    }
                                    (Some(e), None) => {
                                        println!("    Packet {} missing in actual:", i + 1);
                                        println!("      Expected: {}", e);
                                    }
                                    (None, Some(a)) => {
                                        println!("    Packet {} extra in actual:", i + 1);
                                        println!("      Actual:   {}", a);
                                    }
                                    _ => {} // Match, skip
                                }
                            }
                            println!();
                        }
                    }
                    (Some(exp), None) => {
                        mismatched_steps += 1;
                        println!("MISSING Step {}: {} (expected {} packets)", 
                            exp.step_index, exp.step_name, exp.packets.len());
                        println!();
                    }
                    (None, Some(act)) => {
                        mismatched_steps += 1;
                        println!("EXTRA Step {}: {} (got {} packets)", 
                            act.step_index, act.step_name, act.packets.len());
                        println!();
                    }
                    (None, None) => unreachable!(),
                }
            }

            if mismatched_steps == 0 {
                println!("OK: All {} steps match!", actual_steps.len());
            } else {
                println!("FAIL: {} of {} steps differ", mismatched_steps, max_steps);
            }

            println!("\nStopping driver...");
            driver_instance.shutdown()?;
            println!("Done");
        }
    }

    Ok(())
}
