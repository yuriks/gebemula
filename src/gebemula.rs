use timeline::{EventType, Event};

use peripherals::Peripheral;
use peripherals::joypad::{self, Joypad, JoypadKey};
use peripherals::lcd::LCD;

use cpu::{ioregister, interrupt};
use cpu::cpu::{Cpu, Instruction};
use cpu::timer::Timer;

use graphics;

use mem::mem::Memory;
use debugger::Debugger;

use sdl2;
use sdl2::pixels::{PixelFormatEnum, Color};
use sdl2::keyboard::{Scancode, Keycode};

use time;
use std::{self, thread};

pub struct Gebemula<'a> {
    cpu: Cpu,
    mem: Memory,
    timer: Timer,
    debugger: Debugger,
    cycles_per_sec: u32,
    lcd: LCD,
    joypad: Joypad,

    /// Used to periodically save the battery-backed cartridge SRAM to file.
    battery_save_callback: Option<&'a Fn(&[u8])>,
}

impl<'a> Default for Gebemula<'a> {
    fn default() -> Gebemula<'a> {
        Gebemula {
            cpu: Cpu::default(),
            mem: Memory::default(),
            timer: Timer::default(),
            debugger: Debugger::default(),
            cycles_per_sec: 0,
            lcd: LCD::default(),
            joypad: Joypad::default(),
            battery_save_callback: None,
        }
    }
}

impl<'a> Gebemula<'a> {
    pub fn restart(&mut self) {
        self.cpu.restart();
        self.mem.restart();
        self.lcd.restart(&mut self.mem);
        self.timer = Timer::default();
        self.cycles_per_sec = 0;
        self.joypad = Joypad::default();
    }

    pub fn load_bootstrap_rom(&mut self, bootstrap_rom: &[u8]) {
        self.mem.load_bootstrap_rom(bootstrap_rom);
    }

    pub fn load_cartridge(&mut self, game_rom: &[u8], battery: &[u8]) {
        self.mem.load_cartridge(game_rom, battery);
    }

    pub fn set_save_battery_callback(&mut self, callback: &'a Fn(&[u8])) {
        self.battery_save_callback = Some(callback);
    }

    fn update_battery(&mut self) {
        if let Some(ref callback) = self.battery_save_callback {
            let data = self.mem.save_battery();
            if !data.is_empty() {
                callback(&data);
            }
        }
    }

    fn run_event(&mut self, event: Event) {
        match event.event_type {
            EventType::BootstrapFinished => {
                self.mem.disable_bootstrap();
            }
            EventType::DMATransfer => {
                self.mem.set_access_oam(true);
                ioregister::dma_transfer(event.additional_value, &mut self.mem);
                self.mem.set_access_oam(false);
            }
            EventType::JoypadPressed => {
                let buttons = self.joypad.keys(ioregister::joypad_buttons_selected(&self.mem));
                // old buttons & !new_buttons != 0 -> true if there was a change from 1 to 0.
                // new_buttons < 0b1111 -> make sure at least 1 button was pressed.
                if ioregister::joypad_buttons(&self.mem) & !buttons != 0 && buttons < 0b1111 {
                    // interrupt is requested when a button goes from 1 to 0.
                    interrupt::request(interrupt::Interrupt::Joypad, &mut self.mem);
                }

                ioregister::joypad_set_buttons(buttons, &mut self.mem);
            }
        }

    }

    fn step(&mut self) -> u32 {
        let mut cycles: u32 = 0;
        while cycles < self.lcd.stat_mode_duration() {
            //if !ioregister::LCDCRegister::is_lcd_display_enable(&self.mem) {
            //    self.mem.set_access_vram(true);
            //    self.mem.set_access_oam(true);
            //}
            let (instruction, one_event): (Instruction, Option<Event>) =
                                           self.cpu.run_instruction(&mut self.mem);
            self.timer.update(instruction.cycles, &mut self.mem);
            if let Some(e) = one_event {
                self.run_event(e);
                cycles += e.duration;
                self.timer.update(e.duration, &mut self.mem);
            }
            self.cpu.handle_interrupts(&mut self.mem);
            if cfg!(debug_assertions) {
                self.debugger.run(&instruction, &self.cpu, &self.mem, &self.timer);
            }
            cycles += instruction.cycles;
        }
        self.lcd.handle_event(&mut self.mem);
        cycles
    }

    fn set_joypad_key(&mut self, key: JoypadKey, code: Scancode, event_pump: &sdl2::EventPump) {
        if event_pump.keyboard_state().is_scancode_pressed(code) {
            self.joypad.press_key(key);
        } else {
            self.joypad.release_key(key);
        }
    }

    fn adjust_joypad_keys(&mut self, event_pump: &sdl2::EventPump) {
        self.set_joypad_key(joypad::A, Scancode::Z, event_pump);
        self.set_joypad_key(joypad::B, Scancode::X, event_pump);
        self.set_joypad_key(joypad::SELECT, Scancode::LShift, event_pump);
        self.set_joypad_key(joypad::START, Scancode::LCtrl, event_pump);

        self.set_joypad_key(joypad::RIGHT, Scancode::Right, event_pump);
        self.set_joypad_key(joypad::LEFT, Scancode::Left, event_pump);
        self.set_joypad_key(joypad::UP, Scancode::Up, event_pump);
        self.set_joypad_key(joypad::DOWN, Scancode::Down, event_pump);
    }

    fn print_buttons() {
        println!(" Gameboy | Keyboard");
        println!("---------+------------");
        println!("   dir   |  arrows");
        println!("    A    |    Z");
        println!("    B    |    X");
        println!("  start  | left ctrl");
        println!("  select | left shift");
        println!("---------+------------");
        println!("  U: increase speed");
        println!("  I: decrease speed");
        println!("  R: restart");
        println!(" F1: toggle background");
        println!(" F2: toggle window");
        println!(" F3: toggle sprites");
        println!("Tab: speed up while being held down");
        println!("Esc: quit");
        println!("######################");
    }

    pub fn run_sdl(&mut self) {
        Gebemula::print_buttons();

        let sdl_context = sdl2::init().unwrap();
        let vide_subsystem = sdl_context.video().unwrap();

        let window = vide_subsystem.window("Gebemula Emulator",
                                           graphics::consts::DISPLAY_WIDTH_PX as u32 * 2,
                                           graphics::consts::DISPLAY_HEIGHT_PX as u32 * 2)
            .opengl()
            .build()
            .unwrap();

        let mut renderer = window.renderer().build().unwrap();
        renderer.set_draw_color(Color::RGBA(0, 0, 0, 255));

        let mut texture =
            renderer.create_texture_streaming(PixelFormatEnum::ABGR8888,
                                              graphics::consts::DISPLAY_WIDTH_PX as u32,
                                              graphics::consts::DISPLAY_HEIGHT_PX as u32)
            .unwrap();

        renderer.clear();
        renderer.present();

        let mut event_pump = sdl_context.event_pump().unwrap();
        let mut last_time_seconds = time::now();
        let mut last_time = time::now();

        let mut speed_mul: u32 = 1;
        let target_fps: u32 = 60;
        let mut desired_frametime_ns: u32 = 1_000_000_000 / target_fps;
        let mut fps: u32 = 0;
        if !cfg!(debug_assertions) {
            self.debugger.display_info(&self.mem);
        }
        'running: loop {
            for event in event_pump.poll_iter() {
                match event {
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::F1), .. } => {
                        self.lcd.graphics.toggle_bg();
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::F2), .. } => {
                        self.lcd.graphics.toggle_wn();
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::F3), .. } => {
                        self.lcd.graphics.toggle_sprites();
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::Q), .. } => {
                        self.debugger.cancel_run();
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::R), .. } => {
                        self.restart();
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::Tab), repeat: false, .. } => {
                        speed_mul += 1;
                        println!("speed x{}", speed_mul);
                        desired_frametime_ns = 1_000_000_000 / (target_fps*speed_mul);
                    }
                    sdl2::event::Event::KeyUp { keycode: Some(Keycode::Tab), repeat: false, .. } => {
                        speed_mul -= 1;
                        println!("speed x{}", speed_mul);
                        desired_frametime_ns = 1_000_000_000 / (target_fps*speed_mul);
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::U), .. } => {
                        speed_mul += 1;
                        if speed_mul >= 15 {
                            speed_mul = 15;
                        }
                        println!("speed x{}", speed_mul);
                        desired_frametime_ns = 1_000_000_000 / (target_fps*speed_mul);
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::I), .. } => {
                        speed_mul -= 1;
                        if speed_mul == 0 {
                            speed_mul = 1;
                        }
                        println!("speed x{}", speed_mul);
                        desired_frametime_ns = 1_000_000_000 / (target_fps*speed_mul);
                    }
                    sdl2::event::Event::Quit {..} |
                        sdl2::event::Event::KeyDown { keycode: Some(Keycode::Escape), .. } => {
                            break 'running
                        }
                    _ => {}
                }
            }

            self.adjust_joypad_keys(&event_pump);
            self.cycles_per_sec += self.step();

            /*
             * Yuri Kunde Schlesner:
             * it's just the way you do it (fps checking)  seems brittle and
             * you'll get error depending on your timing
             * instead of counting "each >= 1 second check how many frames
             * were rendered and show that as fps", you should either do
             * "each >= 1 second check how many frame were rendered / *actual*
             * elapsed time since last reset of fps"
             * or "each N frames, check elapsed time since last fps update and
             * calculate based on that" fps is just 1 / frametime, so you should
             * just try to average frametime over time to calculate it imo
             *
             * https://github.com/yuriks/super-match-5-dx/blob/master/src/main.cpp#L224
             */
            if self.lcd.has_entered_vblank(&self.mem) {
                renderer.clear();
                texture.update(None, &self.lcd.graphics.screen_buffer,
                               graphics::consts::DISPLAY_WIDTH_PX as usize * 4).unwrap();
                renderer.copy(&texture, None, None);
                renderer.present();

                //clear buffer
                for p in self.lcd.graphics.screen_buffer.chunks_mut(4) {
                    // This actually makes the code faster by skipping redundant bound checking:
                    assert!(p.len() == 4);

                    let color = graphics::consts::DMG_PALETTE[0];
                    p[0] = color.0;
                    p[1] = color.1;
                    p[2] = color.2;
                    p[3] = 255;
                }

                let now = time::now();
                let elapsed: u32 = (now - last_time).num_nanoseconds().unwrap() as u32;
                if elapsed < desired_frametime_ns {
                    thread::sleep(std::time::Duration::new(0, desired_frametime_ns - elapsed));
                }
                last_time = time::now();
                fps += 1;
            }

            let now = time::now();
            if now - last_time_seconds >= time::Duration::seconds(1) {
                last_time_seconds = now;
                let title: &str = &format!("{} Gebemula - {}", fps, self.cycles_per_sec);
                renderer.window_mut().unwrap().set_title(title).unwrap();
                self.cycles_per_sec = 0;
                fps = 0;

                self.update_battery();
            }
        }
        self.update_battery();
    }
}
