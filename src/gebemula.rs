use timeline::{EventType, Event, EventTimeline};

use cpu;
use cpu::ioregister;
use cpu::interrupt;
use cpu::cpu::{Cpu, Instruction};
use cpu::timer::Timer;

use graphics;
use graphics::graphics::Graphics;

use mem::mem::Memory;
use debugger::Debugger;

use sdl2;
use sdl2::pixels::{PixelFormatEnum, Color};
use sdl2::keyboard::{Scancode, Keycode};

use time;
use std;
use std::thread;

pub struct Gebemula {
    cpu: Cpu,
    mem: Memory,
    timer: Timer,
    debugger: Debugger,
    game_rom: Vec<u8>,
    cycles_per_sec: u32,
    graphics: Graphics,
    should_display_screen: bool,
    timeline: EventTimeline,
    joypad: u8, // nibble to the left are direction keys and to the right button keys.
}

impl Default for Gebemula {
    fn default() -> Gebemula {
        Gebemula {
            cpu: Cpu::default(),
            mem: Memory::default(),
            timer: Timer::default(),
            debugger: Debugger::default(),
            game_rom: Vec::new(),
            cycles_per_sec: 0,
            graphics: Graphics::default(),
            should_display_screen: false,
            timeline: EventTimeline::default(),
            joypad: 0,
        }
    }
}

impl Gebemula {
    pub fn restart(&mut self) {
        self.cpu.restart();
        self.mem.restart();
        self.timer = Timer::default();
        self.cycles_per_sec = 0;
        self.graphics.restart();
        self.should_display_screen = false;
        self.timeline = EventTimeline::default();
        self.joypad = 0;
        ioregister::update_stat_reg_mode_flag(0b10, &mut self.mem);
        self.mem.set_access_vram(true);
        self.mem.set_access_oam(false);
    }

    pub fn load_bootstrap_rom(&mut self, bootstrap_rom: &[u8]) {
        self.mem.load_bootstrap_rom(bootstrap_rom);
    }

    pub fn load_game_rom(&mut self, game_rom: &[u8]) {
        for byte in game_rom {
            self.game_rom.push(*byte);
        }
        self.mem.load_game_rom(game_rom);
    }

    fn run_event(&mut self, event: Event) {
        let mut gpu_mode_number: Option<u8> = None;
        match event.event_type {
            EventType::OAM => {
                gpu_mode_number = Some(0b11);
                self.timeline.curr_event_type = EventType::Vram;
                self.mem.set_access_vram(true);
                self.mem.set_access_oam(true);
                self.graphics.update(&mut self.mem);
            }
            EventType::Vram => {
                gpu_mode_number = Some(0b00);
                self.timeline.curr_event_type = EventType::HorizontalBlank;
            }
            EventType::HorizontalBlank => {
                let mut ly: u8 = self.mem.read_byte(cpu::consts::LY_REGISTER_ADDR);
                ly += 1;
                if ly == graphics::consts::DISPLAY_HEIGHT_PX {
                    self.should_display_screen = true;
                    gpu_mode_number = Some(0b01);
                    self.timeline.curr_event_type = EventType::VerticalBlank;
                    interrupt::request(interrupt::Interrupt::VBlank, &mut self.mem);
                } else {
                    self.timeline.curr_event_type = EventType::OAM;
                    gpu_mode_number = Some(0b10);
                }
                self.mem.write_byte(cpu::consts::LY_REGISTER_ADDR, ly);
            }
            EventType::VerticalBlank => {
                let mut ly: u8 = self.mem.read_byte(cpu::consts::LY_REGISTER_ADDR);
                if ly == graphics::consts::DISPLAY_HEIGHT_PX + 10 {
                    self.timeline.curr_event_type = EventType::OAM;
                    gpu_mode_number = Some(0b10);
                    ly = 0;
                } else {
                    self.timeline.curr_event_type = EventType::VerticalBlank;
                    ly += 1;
                }
                self.mem.write_byte(cpu::consts::LY_REGISTER_ADDR, ly);
            }
            EventType::BootstrapFinished => {
                self.mem.disable_bootstrap();
            }
            EventType::DMATransfer => {
                self.mem.set_access_oam(true);
                ioregister::dma_transfer(event.additional_value, &mut self.mem);
                self.mem.set_access_oam(false);
            }
            EventType::JoypadPressed => {
                let buttons: u8 = if ioregister::joypad_buttons_selected(&self.mem) {
                    self.joypad & 0b0000_1111
                } else {
                    self.joypad >> 4
                };

                ioregister::joypad_set_buttons(buttons, &mut self.mem);
            }
        }

        if let Some(gpu_mode) = gpu_mode_number {
            self.mem.set_access_vram(true);
            self.mem.set_access_oam(true);
            // self.mem.set_access_vram(gpu_mode <= 2);
            // self.mem.set_access_oam(gpu_mode <= 1);

            ioregister::update_stat_reg_mode_flag(gpu_mode, &mut self.mem);
        }
        ioregister::update_stat_reg_coincidence_flag(&mut self.mem);
        ioregister::lcdc_stat_interrupt(&mut self.mem);
    }

    fn step(&mut self) -> u32 {
        self.should_display_screen = false;
        let event: Event = self.timeline.curr_event().unwrap();
        let mut cycles: u32 = 0;
        while cycles < event.duration {
            if !ioregister::LCDCRegister::is_lcd_display_enable(&self.mem) {
                self.mem.set_access_vram(true);
                self.mem.set_access_oam(true);
            }
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
        self.run_event(event);
        cycles
    }

    fn adjust_joypad(&mut self, bit: u8, pressed: bool) -> bool {
        self.joypad = if pressed {
            self.joypad & !(1 << bit)
        } else {
            self.joypad | (1 << bit)
        };
        pressed
    }

    // returns true if joypad changed (i.e. some button was pressed or released);
    fn adjust_joypad_buttons(&mut self, event_pump: &sdl2::EventPump) -> bool {
        let mut pressed: bool;
        pressed = self.adjust_joypad(0,
                                     event_pump.keyboard_state().is_scancode_pressed(Scancode::Z));
        pressed |= self.adjust_joypad(1,
                                      event_pump.keyboard_state().is_scancode_pressed(Scancode::X));
        pressed |= self.adjust_joypad(2,
                                      event_pump.keyboard_state()
                                                .is_scancode_pressed(Scancode::LShift));
        pressed |= self.adjust_joypad(3,
                                      event_pump.keyboard_state()
                                                .is_scancode_pressed(Scancode::LCtrl));
        pressed |= self.adjust_joypad(4,
                                      event_pump.keyboard_state()
                                                .is_scancode_pressed(Scancode::Right));
        pressed |= self.adjust_joypad(5,
                                      event_pump.keyboard_state()
                                                .is_scancode_pressed(Scancode::Left));
        pressed |= self.adjust_joypad(6,
                                      event_pump.keyboard_state()
                                                .is_scancode_pressed(Scancode::Up));
        pressed |= self.adjust_joypad(7,
                                      event_pump.keyboard_state()
                                                .is_scancode_pressed(Scancode::Down));

        pressed
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
                                              (graphics::consts::DISPLAY_WIDTH_PX as u32,
                                               graphics::consts::DISPLAY_HEIGHT_PX as u32))
                    .unwrap();

        renderer.clear();
        renderer.present();

        let mut event_pump = sdl_context.event_pump().unwrap();
        let mut last_time_seconds = time::now();
        let mut last_time = time::now();

        self.joypad = 0b1111_1111;
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
                        self.graphics.toggle_bg();
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::F2), .. } => {
                        self.graphics.toggle_wn();
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::F3), .. } => {
                        self.graphics.toggle_sprites();
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::Q), .. } => {
                        self.debugger.cancel_run();
                    }
                    sdl2::event::Event::KeyDown { keycode: Some(Keycode::R), .. } => {
                        self.restart();
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

            if self.adjust_joypad_buttons(&event_pump) {
                interrupt::request(interrupt::Interrupt::Joypad, &mut self.mem);
            }

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
            if self.should_display_screen {
                renderer.clear();
                texture.update(None, &self.graphics.screen_buffer,
                               graphics::consts::DISPLAY_WIDTH_PX as usize * 4).unwrap();
                renderer.copy(&texture, None, None);
                renderer.present();

                //clear buffer
                self.graphics.screen_buffer = [255;
                (graphics::consts::DISPLAY_HEIGHT_PX as usize *
                 graphics::consts::DISPLAY_WIDTH_PX as usize * 4)];
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
                renderer.window_mut().unwrap().set_title(title);
                self.cycles_per_sec = 0;
                fps = 0;
            }
        }
    }
}
