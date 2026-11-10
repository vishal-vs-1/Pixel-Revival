#include "NesTileObserverMesen.hpp"

#include <cstring>

#include "NES/HdPacks/HdData.h"
#include "NesTileObserver.hpp"

namespace {

bool has_transparent_pixel(const uint8_t* tile) noexcept
{
	for(uint32_t row = 0; row < 8; ++row) {
		const uint8_t low = tile[row];
		const uint8_t high = tile[row + 8];
		for(uint32_t bit = 0; bit < 8; ++bit) {
			const uint8_t value = static_cast<uint8_t>(((low >> (7 - bit)) & 1) |
				(((high >> (7 - bit)) & 1) << 1));
			if(value == 0) return true;
		}
	}
	return false;
}

void submit_tile(const HdPpuTileInfo& tile, uint64_t frame, bool sprite) noexcept
{
	if(tile.TileIndex == HdPpuTileInfo::NoTile) return;
	revival::nes::NesTileObservation observation{};
	observation.frame_sequence = frame;
	observation.tile_index = tile.TileIndex;
	observation.palette_colors = tile.PaletteColors;
	std::memcpy(observation.tile_data.data(), tile.TileData, observation.tile_data.size());
	observation.is_chr_ram = tile.IsChrRamTile ? 1 : 0;
	observation.is_sprite = sprite ? 1 : 0;
	observation.transparency_required = sprite && has_transparent_pixel(tile.TileData) ? 1 : 0;
	revival::nes::global_nes_tile_observer().submit(observation);
}

} // namespace

bool rv_nes_observer_needs_raw_tiles() noexcept
{
	return revival::nes::global_nes_tile_observer().enabled();
}

void rv_nes_observer_submit_screen(HdScreenInfo* screen) noexcept
{
	if(screen == nullptr || !rv_nes_observer_needs_raw_tiles()) return;
	const uint64_t frame = screen->FrameNumber;
	revival::nes::global_nes_tile_observer().begin_frame(frame);
	for(uint32_t index = 0; index < 256u * 240u; ++index) {
		const HdPpuPixelInfo& pixel = screen->ScreenTiles[index];
		submit_tile(pixel.Tile, frame, false);
		for(uint32_t sprite = 0; sprite < pixel.SpriteCount && sprite < 4; ++sprite) {
			submit_tile(pixel.Sprite[sprite], frame, true);
		}
	}
}
