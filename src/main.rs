use anyhow::{Result, anyhow};
use btleplug::api::{
    Central, Manager as _, Peripheral as _, ScanFilter, ValueNotification, WriteType,
};
use btleplug::platform::Manager;
use clap::Parser;
use crossterm::event::KeyModifiers;
use crossterm::{ExecutableCommand, event, terminal};
use futures::stream::StreamExt;
use log::info;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const NUS_RX_CHAR_UUID: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e); // Write
const NUS_TX_CHAR_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e); // Notify

/// Nordic UART Service Client app
#[derive(Parser, Debug)]
#[command(version, about, long_about=None)]
struct Args {
    /// BLE device name filter
    #[arg(short, long)]
    name: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    let args = Args::parse();

    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let central = adapters
        .first()
        .ok_or(anyhow!("No bluetooth adapter found"))?;

    info!("Trying to find device (filter: {})", args.name);
    central.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let peripherals = central.peripherals().await?;
    let peripheral = Arc::new(
        peripherals
            .into_iter()
            .find(|p| {
                if let Ok(Some(props)) = futures::executor::block_on(p.properties()) {
                    if let Some(name) = props.local_name {
                        return name.contains(&args.name);
                    }
                }
                false
            })
            .ok_or(anyhow!("Could not find a device with given name"))?,
    );

    peripheral.connect().await?;
    peripheral.discover_services().await?;

    let chars = peripheral.characteristics();
    let rx_char = Arc::new(
        chars
            .iter()
            .find(|c| c.uuid == NUS_RX_CHAR_UUID)
            .expect("RX characteristic not found")
            .clone(),
    );
    let tx_char = chars
        .iter()
        .find(|c| c.uuid == NUS_TX_CHAR_UUID)
        .expect("TX characteristic not found")
        .clone();

    peripheral.subscribe(&tx_char).await?;

    let rx_char = Arc::new(rx_char);

    // Listen for BLE notifications
    let mut notif_stream = peripheral.notifications().await?;

    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    stdout.execute(terminal::EnterAlternateScreen)?;

    let p = peripheral.clone();
    let ch = rx_char.clone();
    tokio::spawn(async move {
        let _ = p
            .write(&ch, &['l' as u8 & 0x1F], WriteType::WithoutResponse)
            .await;
    });

    tokio::spawn(async move {
        while let Some(ValueNotification { value, .. }) = notif_stream.next().await {
            let s = String::from_utf8_lossy(&value);
            print!("{}", s);
            let _ = stdout.flush();
        }
    });

    loop {
        if event::poll(Duration::from_millis(50)).unwrap() {
            if let event::Event::Key(key_event) = event::read().unwrap() {
                let data = match key_event.code {
                    event::KeyCode::Esc => {
                        break;
                    }
                    event::KeyCode::Backspace => Some(b"\x08".to_vec()),
                    event::KeyCode::Char(c) => {
                        let c = if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                            c as u8 & 0x1F
                        } else {
                            c as u8
                        };
                        Some(vec![c])
                    }
                    event::KeyCode::Left => Some(b"\x1b[D".to_vec()),
                    event::KeyCode::Right => Some(b"\x1b[C".to_vec()),
                    event::KeyCode::Up => Some(b"\x1b[A".to_vec()),
                    event::KeyCode::Down => Some(b"\x1b[B".to_vec()),
                    event::KeyCode::Enter => Some(b"\r".to_vec()),
                    event::KeyCode::Tab => Some(b"\t".to_vec()),
                    _ => None,
                };

                if let Some(data) = data {
                    let p = peripheral.clone();
                    let ch = rx_char.clone();
                    tokio::spawn(async move {
                        let _ = p.write(&ch, &data, WriteType::WithoutResponse).await;
                    });
                }
            }
        }
    }

    terminal::disable_raw_mode()?;
    std::io::stdout().execute(terminal::LeaveAlternateScreen)?;
    info!("NUS terminal exited");

    Ok(())
}
