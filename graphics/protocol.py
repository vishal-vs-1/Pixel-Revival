"""Pure v1 packet-envelope and completed-frame bounds validation."""

from dataclasses import dataclass
import struct
from typing import Final


ABI_VERSION: Final = 1
PACKET_MAGIC: Final = 0x31475652
PACKET_HEADER_BYTES: Final = 32
COMMAND_HEADER_BYTES: Final = 8
MAX_PACKET_BYTES: Final = 8 * 1024 * 1024
MAX_QUEUED_BYTES: Final = 64 * 1024 * 1024
MAX_QUEUED_OPS: Final = 8192
MAX_PENDING_FRAMES: Final = 2
MAX_LOGICAL_SOURCE_PIXELS: Final = 4_194_304
MAX_ASPECT_COMPONENT: Final = 16_384
MAX_TEXTURE_BUDGET_BYTES: Final = 512 * 1024 * 1024
FRAME_DESC_BYTES: Final = 56

RESULT_OK: Final = 0
RESULT_INVALID_ARGUMENT: Final = 1
RESULT_UNSUPPORTED_ABI: Final = 2
RESULT_BAD_PACKET: Final = 3
RESULT_QUEUE_FULL: Final = 4
RESULT_WRONG_THREAD: Final = 5
RESULT_STALE_HANDLE: Final = 6
RESULT_RESOURCE_LIMIT: Final = 7
RESULT_NO_CONTEXT: Final = 8
RESULT_GL_FAILURE: Final = 9
RESULT_CLOSING: Final = 10
RESULT_UNSUPPORTED_OPERATION: Final = 11
RESULT_INTERNAL_ERROR: Final = 12

FEATURE_SURFACE_COMMANDS: Final = 0x00000001
FEATURE_COMPLETED_FRAMES: Final = 0x00000002
FEATURE_SCALE2X: Final = 0x00000004
FEATURE_ACTUAL_DEVICE: Final = 0x00000008
FEATURE_SURFACE_SCALE: Final = 0x00000010

INPUT_MODE_COMMANDS: Final = 1
INPUT_MODE_COMPLETED_FRAMES: Final = 2
SCALE_ORIGINAL: Final = 1
SCALE_2X: Final = 2
SCALE_4X: Final = 4

PIXEL_FORMAT_RGBA8: Final = 1
PIXEL_FORMAT_BGRA8: Final = 2
ALPHA_OPAQUE: Final = 1
ALPHA_STRAIGHT: Final = 2
ALPHA_PREMULTIPLIED: Final = 3
ORIGIN_TOP_LEFT: Final = 1

FILTER_NEAREST: Final = 1
FILTER_LINEAR: Final = 2
FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT: Final = 1

OP_CREATE_SURFACE: Final = 1
OP_BEGIN_UPLOAD: Final = 2
OP_UPLOAD_ROWS: Final = 3
OP_END_UPLOAD: Final = 4
OP_FILL: Final = 5
OP_BLIT: Final = 6
OP_COPY: Final = 7
OP_DESTROY_SURFACE: Final = 8
OP_COMMIT_FRAME: Final = 9
OP_CLEAR_FRAME: Final = 10
OP_SCALE_SURFACE: Final = 11
KNOWN_OPCODES: Final = frozenset(range(OP_CREATE_SURFACE, OP_SCALE_SURFACE + 1))

_PACKET_HEADER = struct.Struct("<IHHIIQQ")
_COMMAND_HEADER = struct.Struct("<HHI")
_U32_MAX = 0xFFFFFFFF
_U64_MAX = 0xFFFFFFFFFFFFFFFF


class ProtocolError(ValueError):
    """Raised when input bytes cannot be safely admitted by the v1 protocol."""


@dataclass(frozen=True, slots=True)
class PacketHeader:
    magic: int
    abi_version: int
    header_bytes: int
    total_bytes: int
    command_count: int
    session_epoch: int
    packet_sequence: int


@dataclass(frozen=True, slots=True)
class CommandHeader:
    opcode: int
    flags: int
    command_bytes: int
    offset: int


@dataclass(frozen=True, slots=True)
class ValidatedPacket:
    header: PacketHeader
    commands: tuple[CommandHeader, ...]


@dataclass(frozen=True, slots=True)
class FrameDescriptor:
    struct_bytes: int
    abi_version: int
    frame_sequence: int
    physical_width: int
    physical_height: int
    physical_stride_bytes: int
    logical_width: int
    logical_height: int
    source_scale: int
    desired_scale: int
    pixel_format: int
    alpha_mode: int
    origin: int


def _require_wire_integer(name: str, value: int, maximum: int) -> None:
    if type(value) is not int or not 0 <= value <= maximum:
        raise ProtocolError(f"{name} must fit its unsigned wire integer")


def validate_packet(data: bytes | bytearray | memoryview, expected_epoch: int) -> ValidatedPacket:
    """Validate the complete command envelope without retaining caller memory."""

    _require_wire_integer("expected_epoch", expected_epoch, _U64_MAX)
    try:
        packet = memoryview(data).cast("B")
    except (TypeError, ValueError) as exc:
        raise ProtocolError("packet must be a contiguous bytes-like value") from exc

    packet_length = packet.nbytes
    if packet_length < PACKET_HEADER_BYTES:
        raise ProtocolError("packet is shorter than its header")
    if packet_length > MAX_PACKET_BYTES:
        raise ProtocolError("packet exceeds the v1 byte limit")

    fields = _PACKET_HEADER.unpack_from(packet, 0)
    header = PacketHeader(*fields)
    if header.magic != PACKET_MAGIC:
        raise ProtocolError("packet magic is invalid")
    if header.abi_version != ABI_VERSION or header.header_bytes != PACKET_HEADER_BYTES:
        raise ProtocolError("packet ABI or header size is unsupported")
    if header.total_bytes != packet_length:
        raise ProtocolError("declared packet size does not match supplied bytes")
    if header.command_count > MAX_QUEUED_OPS:
        raise ProtocolError("packet command count exceeds the v1 limit")
    if header.session_epoch != expected_epoch:
        raise ProtocolError("packet belongs to a stale session epoch")
    if header.packet_sequence == 0:
        raise ProtocolError("packet sequence must be nonzero")

    commands: list[CommandHeader] = []
    offset = PACKET_HEADER_BYTES
    for _ in range(header.command_count):
        remaining = header.total_bytes - offset
        if remaining < COMMAND_HEADER_BYTES:
            raise ProtocolError("command header is truncated")

        opcode, flags, command_bytes = _COMMAND_HEADER.unpack_from(packet, offset)
        if opcode not in KNOWN_OPCODES:
            raise ProtocolError("command opcode is unknown")
        if flags != 0:
            raise ProtocolError("reserved command flags must be zero")
        if command_bytes < COMMAND_HEADER_BYTES:
            raise ProtocolError("command size is smaller than its header")
        # Compare against remaining bytes before advancing the untrusted offset.
        if command_bytes > remaining:
            raise ProtocolError("command extends beyond the packet")

        commands.append(CommandHeader(opcode, flags, command_bytes, offset))
        offset += command_bytes

    if offset != header.total_bytes:
        raise ProtocolError("packet contains bytes outside its declared commands")
    return ValidatedPacket(header, tuple(commands))


def validate_frame_payload(
    desc: FrameDescriptor,
    pixel_bytes: int,
    *,
    allow_empty: bool = False,
) -> int:
    """Validate dimensions/stride and return the exact RGBA payload byte count."""

    if not isinstance(desc, FrameDescriptor):
        raise ProtocolError("desc must be a FrameDescriptor")
    _require_wire_integer("struct_bytes", desc.struct_bytes, _U32_MAX)
    _require_wire_integer("abi_version", desc.abi_version, _U32_MAX)
    if desc.struct_bytes != FRAME_DESC_BYTES or desc.abi_version != ABI_VERSION:
        raise ProtocolError("frame descriptor ABI or size is unsupported")
    _require_wire_integer("frame_sequence", desc.frame_sequence, _U64_MAX)
    _require_wire_integer("pixel_bytes", pixel_bytes, _U32_MAX)

    u32_fields = (
        "physical_width",
        "physical_height",
        "physical_stride_bytes",
        "logical_width",
        "logical_height",
        "source_scale",
        "desired_scale",
        "pixel_format",
        "alpha_mode",
        "origin",
    )
    for name in u32_fields:
        _require_wire_integer(name, getattr(desc, name), _U32_MAX)

    if desc.source_scale not in (SCALE_ORIGINAL, SCALE_2X, SCALE_4X):
        raise ProtocolError("source_scale must be 1, 2, or 4")
    if desc.desired_scale not in (SCALE_ORIGINAL, SCALE_2X, SCALE_4X):
        raise ProtocolError("desired_scale must be 1, 2, or 4")
    if desc.pixel_format not in (PIXEL_FORMAT_RGBA8, PIXEL_FORMAT_BGRA8):
        raise ProtocolError("pixel_format is unsupported")
    if desc.alpha_mode not in (ALPHA_OPAQUE, ALPHA_STRAIGHT, ALPHA_PREMULTIPLIED):
        raise ProtocolError("alpha_mode is unsupported")
    if desc.origin != ORIGIN_TOP_LEFT:
        raise ProtocolError("origin is unsupported in v1")

    dimensions = (
        desc.physical_width,
        desc.physical_height,
        desc.physical_stride_bytes,
        desc.logical_width,
        desc.logical_height,
    )
    if all(value == 0 for value in dimensions) and pixel_bytes == 0 and allow_empty:
        return 0
    if any(value == 0 for value in dimensions):
        raise ProtocolError("frame dimensions and stride must all be positive")

    if desc.logical_width * desc.logical_height > MAX_LOGICAL_SOURCE_PIXELS:
        raise ProtocolError("logical frame exceeds the source pixel limit")
    if desc.logical_width * desc.source_scale != desc.physical_width:
        raise ProtocolError("physical width does not match logical width and source scale")
    if desc.logical_height * desc.source_scale != desc.physical_height:
        raise ProtocolError("physical height does not match logical height and source scale")

    minimum_stride = desc.physical_width * 4
    if minimum_stride > _U32_MAX or desc.physical_stride_bytes < minimum_stride:
        raise ProtocolError("physical stride is too small or overflows u32")
    required_bytes = desc.physical_stride_bytes * desc.physical_height
    if required_bytes > _U32_MAX or required_bytes > MAX_QUEUED_BYTES:
        raise ProtocolError("frame payload exceeds the v1 byte limit")
    if pixel_bytes != required_bytes:
        raise ProtocolError("pixel_bytes must exactly match stride times height")
    return required_bytes
