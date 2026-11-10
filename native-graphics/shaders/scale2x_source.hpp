#ifndef REVIVAL_GRAPHICS_SCALE2X_SOURCE_HPP
#define REVIVAL_GRAPHICS_SCALE2X_SOURCE_HPP

namespace revival::graphics::scale2x_shader {

inline constexpr char kVertex[] = R"GLSL(#version 120
void main() {
    gl_Position = gl_Vertex;
}
)GLSL";

inline constexpr char kFragment[] = R"GLSL(#version 120
uniform sampler2D sourceTexture;
uniform vec2 sourceSize;
uniform vec2 outputSize;

vec4 pixelAt(vec2 coordinate) {
    coordinate = clamp(coordinate, vec2(0.0), sourceSize - vec2(1.0));
    vec2 textureCoordinate = (coordinate + vec2(0.5)) / sourceSize;
    textureCoordinate.y = 1.0 - textureCoordinate.y;
    return texture2D(sourceTexture, textureCoordinate);
}

bool sameColor(vec4 first, vec4 second) {
    return all(equal(first, second));
}

void main() {
    vec2 pixel = vec2(floor(gl_FragCoord.x), outputSize.y - 1.0 - floor(gl_FragCoord.y));
    vec2 sourcePixel = floor(pixel * 0.5);
    vec2 subpixel = mod(pixel, 2.0);
    vec4 center = pixelAt(sourcePixel);
    vec4 north = pixelAt(sourcePixel + vec2(0.0, -1.0));
    vec4 west = pixelAt(sourcePixel + vec2(-1.0, 0.0));
    vec4 east = pixelAt(sourcePixel + vec2(1.0, 0.0));
    vec4 south = pixelAt(sourcePixel + vec2(0.0, 1.0));
    vec4 result = center;
    if (!sameColor(north, south) && !sameColor(west, east)) {
        if (subpixel.x < 0.5 && subpixel.y < 0.5 && sameColor(west, north)) result = west;
        else if (subpixel.x >= 0.5 && subpixel.y < 0.5 && sameColor(north, east)) result = east;
        else if (subpixel.x < 0.5 && subpixel.y >= 0.5 && sameColor(west, south)) result = west;
        else if (subpixel.x >= 0.5 && subpixel.y >= 0.5 && sameColor(south, east)) result = east;
    }
    gl_FragColor = result;
}
)GLSL";

} // namespace revival::graphics::scale2x_shader

#endif
