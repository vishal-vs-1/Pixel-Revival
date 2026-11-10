#include "pixel_transfer.hpp"

#include <new>
#include <utility>

#include "frame_desc.hpp"

namespace revival::graphics {

RvResult convert_rows_to_gl_rgba(
	const RvFrameDesc& desc, uint32_t first_row, uint32_t row_count,
	uint32_t source_stride_bytes, const uint8_t* source, uint32_t source_bytes,
	std::vector<uint8_t>* bottom_up_rgba) noexcept
{
	if(bottom_up_rgba == nullptr || source == nullptr || row_count == 0 ||
		first_row > desc.physical_height || row_count > desc.physical_height - first_row) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const uint64_t descriptor_bytes =
		static_cast<uint64_t>(desc.physical_stride_bytes) * desc.physical_height;
	if(descriptor_bytes > UINT32_MAX) return RV_RESULT_RESOURCE_LIMIT;
	const RvResult descriptor_result = validate_frame_desc(desc, static_cast<uint32_t>(descriptor_bytes));
	if(descriptor_result != RV_RESULT_OK) return descriptor_result;
	const uint64_t minimum_stride = static_cast<uint64_t>(desc.physical_width) * 4;
	const uint64_t required_source_bytes = static_cast<uint64_t>(source_stride_bytes) * row_count;
	const uint64_t converted_bytes = minimum_stride * row_count;
	if(source_stride_bytes != desc.physical_stride_bytes || required_source_bytes != source_bytes ||
		required_source_bytes > RV_MAX_PACKET_BYTES || converted_bytes > UINT32_MAX) {
		return required_source_bytes > RV_MAX_PACKET_BYTES || converted_bytes > UINT32_MAX ?
			RV_RESULT_RESOURCE_LIMIT : RV_RESULT_INVALID_ARGUMENT;
	}

	try {
		std::vector<uint8_t> converted(static_cast<size_t>(converted_bytes));
		for(uint32_t row = 0; row < row_count; ++row) {
			const uint8_t* source_row = source + static_cast<size_t>(row) * source_stride_bytes;
			// The first top-left source row belongs at the highest GL row for an FBO surface.
			uint8_t* destination_row = converted.data() +
				static_cast<size_t>(row_count - row - 1) * static_cast<size_t>(minimum_stride);
			for(uint32_t x = 0; x < desc.physical_width; ++x) {
				const uint8_t* pixel = source_row + static_cast<size_t>(x) * 4;
				uint8_t* output = destination_row + static_cast<size_t>(x) * 4;
				uint32_t red = desc.pixel_format == RV_PIXEL_FORMAT_RGBA8 ? pixel[0] : pixel[2];
				uint32_t green = pixel[1];
				uint32_t blue = desc.pixel_format == RV_PIXEL_FORMAT_RGBA8 ? pixel[2] : pixel[0];
				uint32_t alpha = desc.alpha_mode == RV_ALPHA_OPAQUE ? 255u : pixel[3];
				if(alpha == 0) {
					red = 0;
					green = 0;
					blue = 0;
				} else if(desc.alpha_mode == RV_ALPHA_STRAIGHT) {
					red = (red * alpha + 127u) / 255u;
					green = (green * alpha + 127u) / 255u;
					blue = (blue * alpha + 127u) / 255u;
				}
				output[0] = static_cast<uint8_t>(red);
				output[1] = static_cast<uint8_t>(green);
				output[2] = static_cast<uint8_t>(blue);
				output[3] = static_cast<uint8_t>(alpha);
			}
		}
		*bottom_up_rgba = std::move(converted);
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

} // namespace revival::graphics
