#![no_std]
#![no_main]

mod chip;

use esp_hal::main;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("PANIC: {}", info);
    // Nothing left to do on a bare-metal panic: halt here rather than spin
    // on real work, so an empty loop is the correct body, not a mistake.
    #[allow(clippy::empty_loop)]
    loop {}
}

#[main]
fn entry() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    esp_println::println!(
        "somfy-rs firmware: RMT {} MHz / div {} -> 1us ticks; CSN={} GDO0={} GDO2={}",
        chip::RMT_CLOCK_MHZ,
        chip::RMT_CLK_DIVIDER,
        chip::pins::CSN,
        chip::pins::GDO0_TX,
        chip::pins::GDO2_RX,
    );
    // Skeleton proves the build/link path only; real driver work replaces
    // this loop in a later task.
    #[allow(clippy::empty_loop)]
    loop {}
}
