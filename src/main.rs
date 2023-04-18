//! embassy hello world
//!
//! This is an example of running the embassy executor with multiple tasks
//! concurrently.

#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

mod ref_cell_i2c;

use core::cell::RefCell;

use bytes::BytesMut;
use embassy_executor::Executor;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    pubsub::{PubSubChannel, Publisher, Subscriber, WaitResult},
    signal::Signal,
};
use embassy_time::{Duration, Timer, Ticker};
use embedded_hdc1080_rs::Hdc1080;
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
use scd4x::scd4x::Scd4x;
use sht3x::{Repeatability, SHT3x};
use static_cell::StaticCell;
use veml7700::Veml7700;
use futures_util::StreamExt;

use crate::ref_cell_i2c::RefCellBus;


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
async fn read_sht3x(
    mut sensor: SHT3x<&'static RefCellBus<I2C<'static, I2C0>>, Delay>,
    mut publisher: Publisher<'static, CriticalSectionRawMutex, Packet, 10, 2, 2>,
) {
    let mut ticker = Ticker::every(Duration::from_secs(1));

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
async fn read_scd41(
    mut sensor: Scd4x<&'static RefCellBus<I2C<'static, I2C0>>, Delay>,
    mut publisher: Publisher<'static, CriticalSectionRawMutex, Packet, 10, 2, 2>,
) {
    let mut ticker = Ticker::every(Duration::from_secs(1));

    loop {
        let x = sensor.measurement().unwrap();

        let temp = x.temperature;
        let humidity = x.humidity;
        let co2 = x.co2 as f32;

        let reading = Packet::new(vec![
            SensorField {
                sensor_name: proto::SensorName::Scd30.into(), // todo fix
                r#type: Some(Type::Temperature(temp)),
            },
            SensorField {
                sensor_name: proto::SensorName::Scd30.into(),
                r#type: Some(Type::Humidity(humidity)),
            },
            SensorField {
                sensor_name: proto::SensorName::Scd30.into(),
                r#type: Some(Type::Co2(co2)),
            },
        ]);

        publisher.publish(reading).await;
        ticker.next().await;
    }
}

#[embassy_executor::task]
async fn report_sensors(mut subscriber: Subscriber<'static, CriticalSectionRawMutex, Packet, 10, 2, 2>) {

    // async iterator over readings
    // let readings = subscriber.flat_map(|p| p.sensor_fields);

    loop {
        // subscriber is a stream, you can use all the stream adaptors, if you want
        // look at StreamExt for more, thi is an ASYNC ITERATOR
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
                Some(Type::Temperature(t)) => println!("{} temp: {}", field.sensor_name, t),
                Some(Type::Humidity(h)) => println!("{} humid: {}", field.sensor_name, h),
                Some(Type::Co2(co2)) => println!("{} co2: {}", field.sensor_name, co2),
                _ => panic!(),
            }
        }

        // this allocates into our heap
        let mut buf = BytesMut::with_capacity(1024);

        packet.encode(&mut buf);

        println!("{:x}", buf);
    }
}

static EXECUTOR: StaticCell<Executor> = StaticCell::new();
static IO: StaticCell<IO> = StaticCell::new();
static READINGS: PubSubChannel<CriticalSectionRawMutex, Packet, 10, 2, 2> = PubSubChannel::new();
static I2C: StaticCell<RefCellBus<I2C<I2C0>>> = StaticCell::new();

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
    let i2c: &_ = I2C.init(RefCellBus::new(I2C::new(
        peripherals.I2C0,
        sda,
        sck,
        200u32.kHz(),
        &mut system.peripheral_clock_control,
        &clocks,
    )));

    let sht = sht3x::SHT3x::new(i2c, Delay::new(&clocks), sht3x::Address::Low);

    let scd = scd4x::scd4x::Scd4x::new(i2c, Delay::new(&clocks));

    embassy::init(
        &clocks,
        esp32c3_hal::systimer::SystemTimer::new(peripherals.SYSTIMER),
    );

    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner
        .spawn(read_scd41(scd, READINGS.publisher().unwrap()))
        .ok();
        spawner
            .spawn(read_sht3x(sht, READINGS.publisher().unwrap()))
            .ok();
        spawner
            .spawn(report_sensors(READINGS.subscriber().unwrap()))
            .ok();
    });
}
