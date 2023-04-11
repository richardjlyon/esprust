//! embassy hello world
//!
//! This is an example of running the embassy executor with multiple tasks
//! concurrently.

#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use core::time::Duration;

use bytes::BytesMut;
use embassy_executor::Executor;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    pubsub::{PubSubChannel, Publisher, Subscriber, WaitResult},
    signal::Signal,
};
use embassy_time::{Duration, Timer, Ticker};
use esp32c3_hal::{
    clock::ClockControl,
    embassy,
    i2c::I2C,
    peripherals::{Peripherals, I2C0},
    prelude::*,
    timer::TimerGroup,
    Delay, Rtc, IO,
};
use esp_alloc::EspHeap;
use esp_backtrace as _;
use esp_println::println;
use prost::{
    alloc::{string::String, vec},
    Message,
};
use proto::{sensor_field::Type, Packet, SensorField};
use sht3x::{Repeatability, SHT3x};
use static_cell::StaticCell;

mod proto {
    use core::sync::atomic::AtomicI32;

    use prost::alloc::{string::String, vec::Vec};

    include!(concat!(env!("OUT_DIR"), "/kwsensor.rs"));

    impl Packet {
        pub fn new(readings: Vec<SensorField>) -> Self {
            Packet {
                packet_id: 1,
                meta: Some(Meta {
                    device_id: String::from("rust"),
                    firmware: String::from(env!("CARGO_PKG_VERSION")),
                }),
                sensor_fields: readings,
            }
        }
    }
}

#[embassy_executor::task]
async fn read_sensors(
    mut sensor: SHT3x<I2C<'static, I2C0>, Delay>,
    mut publisher: Publisher<'static, CriticalSectionRawMutex, Packet, 10, 2, 2>,
) {
    let ticker = Ticker::every(Duration::from_secs(1));

    loop {
        let x = sensor.measure(Repeatability::High).unwrap();

        let temp = x.temperature as f32 / 100.0;
        let humidity = x.humidity as f32 / 100.0;

        let reading = Packet::new(vec![
            SensorField {
                sensor_name: proto::SensorName::Sht15.into(),
                r#type: Some(Type::Temperature(temp)),
            },
            SensorField {
                sensor_name: proto::SensorName::Sht15.into(),
                r#type: Some(Type::Humidity(humidity)),
            },
        ]);

        publisher.publish(reading).await;
        ticker.next().await;
    }
}

#[embassy_executor::task]
async fn report_sensors(mut subscriber: Subscriber<'static, CriticalSectionRawMutex, Packet, 10, 2, 2>) {
    loop {
        let packet = match subscriber.next_message().await {
            WaitResult::Message(v) => v,
            _ => panic!(),
        };

        let name = packet
            .meta
            .as_ref()
            .map(|m| m.device_id.as_str())
            .unwrap_or("unknown");

        for field in &packet.sensor_fields {
            match field.r#type {
                Some(Type::Temperature(t)) => println!("{} temp: {}", name, t),
                Some(Type::Humidity(h)) => println!("{} humid: {}", name, h),
                _ => panic!(),
                None => panic!(),
            }
        }

        let mut buf = BytesMut::with_capacity(1024);

        packet.encode(&mut buf);

        println!("{:x}", buf);
    }
}

static EXECUTOR: StaticCell<Executor> = StaticCell::new();
static IO: StaticCell<IO> = StaticCell::new();
static READINGS: PubSubChannel<CriticalSectionRawMutex, Packet, 10, 2, 2> = PubSubChannel::new();

#[global_allocator]
static HEAP: EspHeap = EspHeap::empty();

pub fn init_heap() {
    const HEAP_SIZE: usize = 64 * 1024;

    extern "C" {
        static mut _heap_start: u32;
        //static mut _heap_end: u32; // XXX we don't have it on ESP32-C3 currently
    }

    unsafe {
        let heap_start = &_heap_start as *const _ as usize;

        //let heap_end = &_heap_end as *const _ as usize;
        //assert!(heap_end - heap_start > HEAP_SIZE, "Not enough available heap memory.");

        HEAP.init(heap_start as *mut u8, HEAP_SIZE);
    }
}

#[entry]
fn main() -> ! {
    init_heap();

    println!("Init!");
    let peripherals = Peripherals::take();

    let mut system = peripherals.SYSTEM.split();
    let clocks = ClockControl::boot_defaults(system.clock_control).freeze();

    // Disable watchdog timers
    let mut rtc = Rtc::new(peripherals.RTC_CNTL);
    let timer_group0 = TimerGroup::new(peripherals.TIMG0, &clocks);
    let mut wdt0 = timer_group0.wdt;
    let timer_group1 = TimerGroup::new(peripherals.TIMG1, &clocks);
    let mut wdt1 = timer_group1.wdt;

    rtc.swd.disable();
    rtc.rwdt.disable();
    wdt0.disable();
    wdt1.disable();
    // End disable watchdog

    let io = IO.init(IO::new(peripherals.GPIO, peripherals.IO_MUX));
    let sda = io
        .pins
        .gpio1
        .internal_pull_up(true)
        .set_to_open_drain_output()
        .set_alternate_function(esp32c3_hal::gpio::AlternateFunction::Function4);
    let sck = io
        .pins
        .gpio2
        .internal_pull_up(true)
        .set_to_open_drain_output()
        .set_alternate_function(esp32c3_hal::gpio::AlternateFunction::Function4);

    // Create a new peripheral object with the described wiring
    // and standard I2C clock speed
    let i2c = I2C::new(
        peripherals.I2C0,
        sda,
        sck,
        200u32.kHz(),
        &mut system.peripheral_clock_control,
        &clocks,
    );

    // let mut sensor = Veml7700::new(i2c);
    // sensor.enable().unwrap();

    let sensor = sht3x::SHT3x::new(i2c, Delay::new(&clocks), sht3x::Address::Low);

    // let mut sensor = Hdc1080::new(i2c, Delay::new(&clocks)).unwrap();
    // sensor.init().unwrap();

    // esp_println::println!("reading! {:?}", sensor.read().unwrap());

    embassy::init(
        &clocks,
        esp32c3_hal::systimer::SystemTimer::new(peripherals.SYSTIMER),
    );

    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner
            .spawn(read_sensors(sensor, READINGS.publisher().unwrap()))
            .ok();
        spawner
            .spawn(report_sensors(READINGS.subscriber().unwrap()))
            .ok();
    });
}
