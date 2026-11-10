#pragma once

struct HdScreenInfo;

bool rv_nes_observer_needs_raw_tiles() noexcept;
void rv_nes_observer_submit_screen(HdScreenInfo* screen) noexcept;
