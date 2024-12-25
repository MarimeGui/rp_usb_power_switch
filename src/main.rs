#![no_std]
#![no_main]

use defmt::info;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_rp::{
    bind_interrupts,
    gpio::{Level::Low, Output},
    init,
    peripherals::USB,
    usb::{Driver, InterruptHandler},
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    watch::{Sender, Watch},
};
use embassy_time::Timer;
use embassy_usb::{
    class::hid::{Config as HIDConfig, HidReaderWriter, ReportId, RequestHandler, State},
    control::OutResponse,
    Builder, Config, Handler,
};
use panic_probe as _;

// -----

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

const MAX_DURATION: u32 = 1000 * 60 * 10; // 10 Minutes

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = init(Default::default());

    info!("Let's go");

    // Pin
    let mut relay_pin = Output::new(p.PIN_10, Low);

    // Setup task to toggle pin
    let watch = Watch::<CriticalSectionRawMutex, u32, 4>::new();
    let mut receiver = watch.receiver().unwrap();
    let transmitter = watch.sender();
    let toggle_task = async {
        loop {
            let wait_for = receiver.changed().await;
            info!("Wait for {}ms", wait_for);
            relay_pin.set_high();
            Timer::after_millis(wait_for as u64).await;
            info!("Finished waiting !");
            relay_pin.set_low();
            receiver.try_changed(); // Force values sent while waiting to get discarded
        }
    };

    // Setup USB
    let driver = Driver::new(p.USB, Irqs);
    let mut config = Config::new(0xc0de, 0xcafe);
    config.manufacturer = Some("MarimeGui");
    config.product = Some("USB Momentary Power Switch");
    config.serial_number = Some("The Only One");
    config.max_power = 100;
    config.max_packet_size_0 = 64;
    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut msos_descriptor = [0; 256];
    let mut control_buf = [0; 64];
    let mut request_handler = MyRequestHandler { tx: transmitter };
    let mut device_handler = MyDeviceHandler {};
    let mut state = State::new();
    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buf,
    );
    builder.handler(&mut device_handler);
    let config = HIDConfig {
        report_descriptor: HID_DESCRIPTOR,
        request_handler: None,
        poll_ms: 60,
        max_packet_size: 64,
    };
    let hid = HidReaderWriter::<_, 4, 0>::new(&mut builder, &mut state, config);
    let mut usb = builder.build();
    let usb_fut = usb.run();
    let (reader, mut _writer) = hid.split();

    info!("Now waiting");

    // Wait for anything coming in
    join(
        usb_fut,
        join(reader.run(false, &mut request_handler), toggle_task),
    )
    .await;
}

// ----- HID Descriptor

#[rustfmt::skip]
const HID_DESCRIPTOR: &[u8] = &[
    0x5, 1,     // Usage Page: Generic Desktop
    0x9, 0,     // Usage: Undefined
    0xA1, 1,    // Collection: Application
    0x15, 0,    // Logical Minimum: 0
    0x27, 0xFF, 0xFF, 0xFF, 0xFF, // Logical Maximum: 0xFFFFFFFF
    0x85, 1,    // Report ID: 1
    0x75, 0x20, // Report Size: 32 bits
    0x95, 1,    // Report Count: 1
    0x9, 0,     // Usage: Undefined
    0x81, 0x82, // Input: Variable, Volatile
    0xC0        // End Collection
];

// ----- Request Handler

struct MyRequestHandler<'a> {
    tx: Sender<'a, CriticalSectionRawMutex, u32, 4>,
}

impl RequestHandler for MyRequestHandler<'_> {
    // This is where data sent from computer will end up
    fn set_report(&mut self, id: ReportId, data: &[u8]) -> OutResponse {
        info!("Report {}, Received {:?}", id, data);

        // Make sure this is a 4-byte value
        let duration_bytes = match data.try_into() {
            Ok(v) => v,
            Err(_) => return OutResponse::Rejected,
        };

        // Convert to a u32
        let duration = u32::from_be_bytes(duration_bytes);

        // Only process values in range
        if (duration > 0) & (duration <= MAX_DURATION) {
            self.tx.send(duration);
        }

        OutResponse::Accepted
    }
}

// ----- Device Handler

struct MyDeviceHandler {}

impl Handler for MyDeviceHandler {}
