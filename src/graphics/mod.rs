pub mod consts;

use super::util;
use super::mem::Memory;
use super::cpu::ioregister;
use super::cpu;
use super::gebemula::GBMode;

#[derive(Copy, Clone, PartialEq)]
enum Priority {
    Sprite,
    Background
}
// Tile attributes
#[derive(Copy, Clone)]
struct TileAttr(u8);
impl TileAttr {
    pub fn cgb_palette_number(&self) -> u8 {
        self.0 & 0b111
    }
    pub fn tile_vram_bank(&self) -> u8 {
        (self.0 >> 3) & 0b1
    }
    pub fn dmg_palette_number(&self) -> u8 {
        (self.0 >> 4) & 0b1
    }
    pub fn h_flip(&self) -> bool {
        ((self.0 >> 5) & 0b1) == 0b1
    }
    pub fn v_flip(&self) -> bool {
        ((self.0 >> 6) & 0b1) == 0b1
    }
    pub fn priority(&self) -> Priority {
        match (self.0 >> 7) & 0b1 {
            0 => Priority::Sprite,
            1 => Priority::Background,
            _ => unreachable!(),
        }
    }
}

#[derive(Copy, Clone)]
struct TilePixel {
    // color number in the palette for the pixel (0-3).
    pub color_number: u8,
    pub tile_attr: TileAttr,
}
impl TilePixel {
    pub fn new(color_number: u8, tile_attr: TileAttr) -> Self {
        TilePixel {
            color_number: color_number,
            tile_attr: tile_attr,
        }
    }
}
impl Default for TilePixel {
    fn default() -> Self {
        TilePixel {
            color_number: 0,
            tile_attr: TileAttr(0),
        }
    }
}

pub struct Graphics {
    bg_wn_pixel_indexes: [TilePixel; 160 * 144],
    pub screen_buffer: [u8; 160 * 144 * 4],
    bg_on: bool,
    wn_on: bool,
    sprites_on: bool,
}

impl Default for Graphics {
    fn default() -> Graphics {
        Graphics {
            screen_buffer: [255; 160 * 144 * 4],
            bg_wn_pixel_indexes: [TilePixel::default(); 160 * 144],
            bg_on: true,
            wn_on: true,
            sprites_on: true,
        }
    }
}

impl Graphics {
    pub fn restart(&mut self) {
        self.screen_buffer = [255; 160 * 144 * 4];
        self.bg_wn_pixel_indexes = [TilePixel::default(); 160 * 144];
        self.bg_on = true;
        self.wn_on = true;
        self.sprites_on = true;
    }

    pub fn update(&mut self, memory: &mut Memory) {
        if ioregister::LCDCRegister::is_lcd_display_enable(memory) {
            self.update_line_buffer(memory);
            self.draw_sprites(memory);
        }
    }

    fn rgb(palette_h: u8, palette_l: u8) -> (u8, u8, u8) {
        let r = palette_l & 0b0001_1111;
        let g = ((palette_h & 0b11) << 3) | (palette_l >> 5);
        let b = (palette_h >> 2) & 0b11111;

        let to255 = |x| {
            (x << 3) | (x >> 2)
        };

        (to255(r), to255(g), to255(b))
    }
    fn bg_rgb(palette_num: u8, color_number:u8, memory: &Memory) -> (u8, u8, u8) {
        let palette_h = memory.read_bg_palette((palette_num * 8) + 1 + (color_number * 2)); //each palette uses 8 bytes.
        let palette_l = memory.read_bg_palette((palette_num * 8) + (color_number * 2)); // pixel_data chooses the palette index. *2 because each color intensity uses two bytes.
        Graphics::rgb(palette_h, palette_l)
    }
    fn sprite_rgb(palette_num: u8, color_number:u8, memory: &Memory) -> (u8, u8, u8) {
        let palette_h = memory.read_sprite_palette((palette_num * 8) + 1 + (color_number * 2)); //each palette uses 8 bytes.
        let palette_l = memory.read_sprite_palette((palette_num * 8) + (color_number * 2)); // pixel_data chooses the palette index. *2 because each color intensity uses two bytes.
        Graphics::rgb(palette_h, palette_l)
    }

    fn update_line_buffer(&mut self, memory: &mut Memory) {
        let mode_color = GBMode::get(memory) == GBMode::Color;

        let mut bg_on = ioregister::LCDCRegister::is_bg_window_display_on(memory);
        let mut wn_on = ioregister::LCDCRegister::is_window_display_on(memory);

        bg_on = if mode_color { self.bg_on } else { bg_on && self.bg_on };
        wn_on = wn_on && self.wn_on;

        if !bg_on && !wn_on {
            return;
        }

        let curr_line = memory.read_byte(cpu::consts::LY_REGISTER_ADDR);
        if curr_line >= consts::DISPLAY_HEIGHT_PX {
            return;
        }
        let scx = memory.read_byte(cpu::consts::SCX_REGISTER_ADDR);
        let scy = memory.read_byte(cpu::consts::SCY_REGISTER_ADDR);
        let mut ypos = curr_line.wrapping_add(scy) as u16;
        let wy = memory.read_byte(cpu::consts::WY_REGISTER_ADDR);
        let wx = memory.read_byte(cpu::consts::WX_REGISTER_ADDR).wrapping_sub(7);

        let mut is_window = false;

        let startx = if bg_on {
            0
        } else {
            wx
        };

        let old_vbk = memory.read_byte(cpu::consts::VBK_REGISTER_ADDR);

        let (tile_table_addr_pattern_0, is_tile_number_signed) =
            if ioregister::LCDCRegister::is_tile_data_0(&memory) {
                (consts::TILE_DATA_TABLE_0_ADDR_START, true)
            } else {
                (consts::TILE_DATA_TABLE_1_ADDR_START, false)
            };


        let mut tile_row = (ypos / 8) * 32;
        let mut tile_line = (ypos % 8) * 2;
        for i in startx..consts::DISPLAY_WIDTH_PX {
            if wn_on && !is_window && i >= wx && wx < consts::DISPLAY_WIDTH_PX && curr_line >= wy {
                is_window = true;
                ypos = (curr_line - wy) as u16;
                tile_row = (ypos / 8) * 32;
                tile_line = (ypos % 8) * 2;
            }

            let xpos = if is_window {
                i.wrapping_sub(wx) as u16
            } else {
                scx.wrapping_add(i) as u16
            };

            let buffer_pos = (curr_line as usize * consts::DISPLAY_WIDTH_PX as usize) +
                    (i as usize);

            if !bg_on && !is_window {
                self.bg_wn_pixel_indexes[buffer_pos] = TilePixel::default();
                continue;
            }

            let addr_start = if is_window {
                if ioregister::LCDCRegister::is_window_tile_map_display_normal(&memory) {
                    consts::BG_NORMAL_ADDR_START
                } else {
                    consts::BG_WINDOW_ADDR_START
                }
            } else if ioregister::LCDCRegister::is_bg_tile_map_display_normal(&memory) {
                consts::BG_NORMAL_ADDR_START
            } else {
                consts::BG_WINDOW_ADDR_START
            };

            let tile_col_bg = xpos >> 3;
            let tile_addr = addr_start + tile_row + tile_col_bg;
            let mut tile_col = xpos % 8;

            // tile map is on vram bank 0
            memory.write_byte(cpu::consts::VBK_REGISTER_ADDR, 0);
            let tile_location = if is_tile_number_signed {
                let mut tile_number = util::sign_extend(memory.read_byte(tile_addr));
                if util::is_neg16(tile_number) {
                    tile_number = 128 - util::twos_complement(tile_number);
                } else {
                    tile_number += 128;
                }
                tile_table_addr_pattern_0 + (tile_number * consts::TILE_SIZE_BYTES as u16)
            } else {
                tile_table_addr_pattern_0 +
                (memory.read_byte(tile_addr) as u16 * consts::TILE_SIZE_BYTES as u16)
            };

            let mut attr = None;
            if mode_color {
                // tile attribute is on vram bank 1
                memory.write_byte(cpu::consts::VBK_REGISTER_ADDR, 1);

                attr = Some(TileAttr(memory.read_byte(tile_addr)));

                if attr.unwrap().h_flip() {
                    tile_col = 7 - tile_col;
                }
                if attr.unwrap().v_flip() {
                    tile_line = 15 - tile_line;
                }
                // set vbk to use the correct bank for the tile data.
                memory.write_byte(cpu::consts::VBK_REGISTER_ADDR, attr.unwrap().tile_vram_bank());
            }

            // two bytes representing 8 pixel indexes
            let lhs = memory.read_byte(tile_location + tile_line) >> (7 - tile_col);
            let rhs = memory.read_byte(tile_location + tile_line + 1) >> (7 - tile_col);

            let color_number = ((rhs << 1) & 0b10) | (lhs & 0b01);

            self.bg_wn_pixel_indexes[buffer_pos] = TilePixel::new(color_number, attr.unwrap_or(TileAttr(0)));
            if let Some(attr) = attr {
                let (r, g, b) = Graphics::bg_rgb(attr.cgb_palette_number(), color_number, memory);

                // bit 0 of LCDC (bg_on) takes priority over the tile's priority attribute.
                let buffer_pos = buffer_pos * 4; //*4 because of RGBA
                self.screen_buffer[buffer_pos] = r;
                self.screen_buffer[buffer_pos + 1] = g;
                self.screen_buffer[buffer_pos + 2] = b;
                self.screen_buffer[buffer_pos + 3] = 255; //alpha
            } else {
                memory.write_byte(cpu::consts::VBK_REGISTER_ADDR, 0);
                let pixel_index = ioregister::bg_window_palette(color_number, memory);
                // Apply palette
                let (r, g, b) = consts::DMG_PALETTE[pixel_index as usize];

                let buffer_pos = buffer_pos * 4; //*4 because of RGBA
                self.screen_buffer[buffer_pos] = r;
                self.screen_buffer[buffer_pos + 1] = g;
                self.screen_buffer[buffer_pos + 2] = b;
                self.screen_buffer[buffer_pos + 3] = 255; //alpha
            }
        }
        memory.write_byte(cpu::consts::VBK_REGISTER_ADDR, old_vbk);
    }

    fn draw_sprites(&mut self, memory: &mut Memory) {
        // TODO draw sprites based on X priority. (only for Non-CGB)
        if !ioregister::LCDCRegister::is_sprite_display_on(memory) || !self.sprites_on {
            return;
        }

        let curr_line = memory.read_byte(cpu::consts::LY_REGISTER_ADDR);
        if curr_line >= consts::DISPLAY_HEIGHT_PX || !self.sprites_on {
            return;
        }

        let mode_color = GBMode::get(memory) == GBMode::Color;
        let old_vbk = memory.read_byte(cpu::consts::VBK_REGISTER_ADDR);

        let mut index = 160; //40*4: 40 sprites that use 4 bytes
        while index != 0 {
            index -= 4;
            let sprite_8_16 = ioregister::LCDCRegister::is_sprite_8_16_on(memory);
            let height = if sprite_8_16 {
                16
            } else {
                8
            };
            let mut y = memory.read_byte(consts::SPRITE_ATTRIBUTE_TABLE + index) as i16;
            if y == 0 || y >= 160 {
                continue;
            }
            y -= 16;
            if ((curr_line as i16) < y) || (curr_line as i16 >= y + height as i16) {
                // outside sprite
                continue;
            }

            let mut x = memory.read_byte(consts::SPRITE_ATTRIBUTE_TABLE + index + 1) as i16;
            if x == 0 || x >= 168 {
                continue;
            }

            let tile_number = memory.read_byte(consts::SPRITE_ATTRIBUTE_TABLE + index + 2);

            let tile_location = consts::SPRITE_PATTERN_TABLE_ADDR_START +
                                     (tile_number as u16 * consts::TILE_SIZE_BYTES as u16);

            let sprite_attr = TileAttr(memory.read_byte(consts::SPRITE_ATTRIBUTE_TABLE + index + 3));
            if mode_color {
                // tile attribute is on vram bank 1
                memory.write_byte(cpu::consts::VBK_REGISTER_ADDR, sprite_attr.tile_vram_bank());
            }

            x -= 8;
            let endx = if x + 8 >= consts::DISPLAY_WIDTH_PX as i16 {
                consts::DISPLAY_WIDTH_PX.wrapping_sub(x as u8)
            } else {
                8
            };
            let mut tile_line = (curr_line as i16 - y) as u8;
            for tile_col in 0..endx {
                let mut buffer_pos = (curr_line as usize * consts::DISPLAY_WIDTH_PX as usize) + (x.wrapping_add(tile_col as i16) as u16) as usize;
                if buffer_pos * 4 > self.screen_buffer.len() - 4 {
                    continue;
                }

                let mut tile_col = tile_col;
                if sprite_attr.h_flip() {
                    tile_col = 7 - tile_col;
                }
                if sprite_attr.v_flip() {
                    tile_line = height - 1 - tile_line;
                }
                // tile_line*2 because each tile uses 2 bytes per line.
                let lhs = memory.read_byte(tile_location + (tile_line as u16 * 2)) >>
                              (7 - tile_col);
                let rhs = memory.read_byte(tile_location + (tile_line as u16 * 2) + 1) >>
                              (7 - tile_col);
                let color_number = ((rhs << 1) & 0b10) | (lhs & 0b01);
                if color_number == 0 {
                    continue;
                }

                let sprite_priority = sprite_attr.priority() == Priority::Sprite;
                if mode_color {
                    let sprites_on_top = !ioregister::LCDCRegister::is_bg_window_display_on(memory);
                    let bg_px = self.bg_wn_pixel_indexes[buffer_pos];

                    let oam_priority = bg_px.tile_attr.priority() == Priority::Sprite;

                    if sprites_on_top || (oam_priority && (sprite_priority || bg_px.color_number == 0)) {
                        let (r, g, b) = Graphics::sprite_rgb(sprite_attr.cgb_palette_number(), color_number, memory);

                        buffer_pos *= 4; // because of RGBA
                        self.screen_buffer[buffer_pos] = r;
                        self.screen_buffer[buffer_pos + 1] = g;
                        self.screen_buffer[buffer_pos + 2] = b;
                        self.screen_buffer[buffer_pos + 3] = 255; //alpha
                    }
                } else if sprite_priority || self.bg_wn_pixel_indexes[buffer_pos].color_number == 0 {
                    let pixel_index = ioregister::sprite_palette(sprite_attr.dmg_palette_number() == 0, color_number, memory);
                    let (r, g, b) = consts::DMG_PALETTE[pixel_index as usize];

                    buffer_pos *= 4;
                    self.screen_buffer[buffer_pos] = r;
                    self.screen_buffer[buffer_pos + 1] = g;
                    self.screen_buffer[buffer_pos + 2] = b;
                    self.screen_buffer[buffer_pos + 3] = 255;
                }
            }
        }
        memory.write_byte(cpu::consts::VBK_REGISTER_ADDR, old_vbk);
    }

    pub fn toggle_bg(&mut self) {
        self.bg_on = !self.bg_on;
        println!("bg: {}", self.bg_on);
    }
    pub fn toggle_wn(&mut self) {
        self.wn_on = !self.wn_on;
        println!("wn: {}", self.wn_on);
    }
    pub fn toggle_sprites(&mut self) {
        self.sprites_on = !self.sprites_on;
        println!("sprites: {}", self.sprites_on);
    }
}
