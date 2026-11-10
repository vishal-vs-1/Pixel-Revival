import ctypes
import json
import re
from pathlib import Path

import pytest

from graphics.policy import GraphicsCapabilities, GraphicsPolicy
from graphics.protocol import (
    ABI_VERSION,
    FRAME_DESC_BYTES,
    FEATURE_ACTUAL_DEVICE,
    FEATURE_COMPLETED_FRAMES,
    FEATURE_SCALE2X,
    FEATURE_SURFACE_COMMANDS,
    FEATURE_SURFACE_SCALE,
    MAX_ASPECT_COMPONENT,
    MAX_LOGICAL_SOURCE_PIXELS,
    OP_CLEAR_FRAME,
    PACKET_HEADER_BYTES,
    PACKET_MAGIC,
    RESULT_BAD_PACKET,
    RESULT_CLOSING,
    RESULT_GL_FAILURE,
    RESULT_INVALID_ARGUMENT,
    RESULT_INTERNAL_ERROR,
    RESULT_NO_CONTEXT,
    RESULT_OK,
    RESULT_QUEUE_FULL,
    RESULT_RESOURCE_LIMIT,
    RESULT_STALE_HANDLE,
    RESULT_UNSUPPORTED_ABI,
    RESULT_UNSUPPORTED_OPERATION,
    RESULT_WRONG_THREAD,
    FrameDescriptor,
    ProtocolError,
    validate_frame_payload,
    validate_packet,
)


ROOT = Path(__file__).resolve().parents[1]
HEADER = ROOT / "native-graphics" / "include" / "revival_graphics.h"
FIXTURES = ROOT / "tests" / "fixtures" / "graphics_v1_packets.json"
BUILD_ID = "a" * 64


def _header_constant(name):
    source = HEADER.read_text(encoding="utf-8")
    match = re.search(rf"^#define {name} (?:UINT(?:32|64)_C\()?([0-9A-Fa-fx]+)\)?u?$", source, re.M)
    assert match, f"missing C header constant {name}"
    return int(match.group(1), 0)


def _fixtures():
    return json.loads(FIXTURES.read_text(encoding="utf-8"))


class _BuildInfo(ctypes.Structure):
    _fields_ = [("struct_bytes", ctypes.c_uint32), ("abi_version", ctypes.c_uint32),
                ("build_id", ctypes.c_uint8 * 64), ("feature_bits", ctypes.c_uint32)]


class _AttachConfig(ctypes.Structure):
    _fields_ = [("struct_bytes", ctypes.c_uint32), ("abi_version", ctypes.c_uint32),
                ("session_epoch", ctypes.c_uint64), ("texture_budget_bytes", ctypes.c_uint64),
                ("max_queued_bytes", ctypes.c_uint32), ("max_queued_ops", ctypes.c_uint32),
                ("max_pending_frames", ctypes.c_uint32), ("detail_scale", ctypes.c_uint32),
                ("input_mode", ctypes.c_uint32), ("reserved_zero", ctypes.c_uint32)]


class _FrameDesc(ctypes.Structure):
    _fields_ = [("struct_bytes", ctypes.c_uint32), ("abi_version", ctypes.c_uint32),
                ("frame_sequence", ctypes.c_uint64), ("physical_width", ctypes.c_uint32),
                ("physical_height", ctypes.c_uint32), ("physical_stride_bytes", ctypes.c_uint32),
                ("logical_width", ctypes.c_uint32), ("logical_height", ctypes.c_uint32),
                ("source_scale", ctypes.c_uint32), ("desired_scale", ctypes.c_uint32),
                ("pixel_format", ctypes.c_uint32), ("alpha_mode", ctypes.c_uint32),
                ("origin", ctypes.c_uint32)]


class _Viewport(ctypes.Structure):
    _fields_ = [("struct_bytes", ctypes.c_uint32), ("abi_version", ctypes.c_uint32),
                ("framebuffer_width", ctypes.c_uint32), ("framebuffer_height", ctypes.c_uint32),
                ("aspect_num", ctypes.c_uint32), ("aspect_den", ctypes.c_uint32),
                ("filter", ctypes.c_uint32), ("fit", ctypes.c_uint32)]


class _ReadbackRect(ctypes.Structure):
    _fields_ = [("struct_bytes", ctypes.c_uint32), ("abi_version", ctypes.c_uint32),
                ("x", ctypes.c_uint32), ("y", ctypes.c_uint32), ("width", ctypes.c_uint32),
                ("height", ctypes.c_uint32), ("output_format", ctypes.c_uint32),
                ("output_alpha_mode", ctypes.c_uint32)]


class _PumpStats(ctypes.Structure):
    _fields_ = [("struct_bytes", ctypes.c_uint32), ("abi_version", ctypes.c_uint32),
                ("operations_processed", ctypes.c_uint32), ("frames_completed", ctypes.c_uint32),
                ("queued_bytes", ctypes.c_uint32), ("queued_ops", ctypes.c_uint32),
                ("uploaded_bytes", ctypes.c_uint64), ("elapsed_ns", ctypes.c_uint64)]


class _Stats(ctypes.Structure):
    _fields_ = [("struct_bytes", ctypes.c_uint32), ("abi_version", ctypes.c_uint32),
                ("packets_submitted", ctypes.c_uint64), ("packets_rejected", ctypes.c_uint64),
                ("operations_executed", ctypes.c_uint64), ("bytes_uploaded", ctypes.c_uint64),
                ("frames_presented", ctypes.c_uint64), ("queued_bytes", ctypes.c_uint32),
                ("queued_ops", ctypes.c_uint32), ("pending_frames", ctypes.c_uint32),
                ("reserved_zero", ctypes.c_uint32)]


@pytest.mark.parametrize(
    ("structure", "size_macro", "fields"),
    [
        (_BuildInfo, "RV_BUILD_INFO_BYTES", {"struct_bytes": 0, "abi_version": 4,
                                               "build_id": 8, "feature_bits": 72}),
        (_AttachConfig, "RV_ATTACH_CONFIG_BYTES", {"struct_bytes": 0, "abi_version": 4,
            "session_epoch": 8, "texture_budget_bytes": 16, "max_queued_bytes": 24,
            "max_queued_ops": 28, "max_pending_frames": 32, "detail_scale": 36,
            "input_mode": 40, "reserved_zero": 44}),
        (_FrameDesc, "RV_FRAME_DESC_BYTES", {"struct_bytes": 0, "abi_version": 4,
            "frame_sequence": 8, "physical_width": 16, "physical_height": 20,
            "physical_stride_bytes": 24, "logical_width": 28, "logical_height": 32,
            "source_scale": 36, "desired_scale": 40, "pixel_format": 44,
            "alpha_mode": 48, "origin": 52}),
        (_Viewport, "RV_VIEWPORT_BYTES", {"struct_bytes": 0, "abi_version": 4,
            "framebuffer_width": 8, "framebuffer_height": 12, "aspect_num": 16,
            "aspect_den": 20, "filter": 24, "fit": 28}),
        (_ReadbackRect, "RV_READBACK_RECT_BYTES", {"struct_bytes": 0, "abi_version": 4,
            "x": 8, "y": 12, "width": 16, "height": 20, "output_format": 24,
            "output_alpha_mode": 28}),
        (_PumpStats, "RV_PUMP_STATS_BYTES", {"struct_bytes": 0, "abi_version": 4,
            "operations_processed": 8, "frames_completed": 12, "queued_bytes": 16,
            "queued_ops": 20, "uploaded_bytes": 24, "elapsed_ns": 32}),
        (_Stats, "RV_STATS_BYTES", {"struct_bytes": 0, "abi_version": 4,
            "packets_submitted": 8, "packets_rejected": 16, "operations_executed": 24,
            "bytes_uploaded": 32, "frames_presented": 40, "queued_bytes": 48,
            "queued_ops": 52, "pending_frames": 56, "reserved_zero": 60}),
    ],
)
def test_modeled_abi_layout_matches_frozen_header(structure, size_macro, fields):
    assert ctypes.sizeof(structure) == _header_constant(size_macro)
    for field, expected_offset in fields.items():
        assert getattr(structure, field).offset == expected_offset


def test_wire_constants_match_native_header():
    assert _header_constant("RV_ABI_VERSION") == ABI_VERSION
    assert _header_constant("RV_PACKET_MAGIC") == PACKET_MAGIC
    assert _header_constant("RV_PACKET_HEADER_BYTES") == PACKET_HEADER_BYTES
    assert _header_constant("RV_COMMAND_HEADER_BYTES") == 8
    assert _header_constant("RV_FRAME_DESC_BYTES") == FRAME_DESC_BYTES
    assert _header_constant("RV_OP_CLEAR_FRAME") == OP_CLEAR_FRAME
    assert _header_constant("RV_MAX_ASPECT_COMPONENT") == MAX_ASPECT_COMPONENT
    assert _header_constant("RV_MAX_LOGICAL_SOURCE_PIXELS") == MAX_LOGICAL_SOURCE_PIXELS
    assert _header_constant("RV_MAX_TEXTURE_BUDGET_BYTES") == 512 * 1024 * 1024


@pytest.mark.parametrize(
    ("header_name", "python_value"),
    [
        ("RV_RESULT_OK", RESULT_OK),
        ("RV_RESULT_INVALID_ARGUMENT", RESULT_INVALID_ARGUMENT),
        ("RV_RESULT_UNSUPPORTED_ABI", RESULT_UNSUPPORTED_ABI),
        ("RV_RESULT_BAD_PACKET", RESULT_BAD_PACKET),
        ("RV_RESULT_QUEUE_FULL", RESULT_QUEUE_FULL),
        ("RV_RESULT_WRONG_THREAD", RESULT_WRONG_THREAD),
        ("RV_RESULT_STALE_HANDLE", RESULT_STALE_HANDLE),
        ("RV_RESULT_RESOURCE_LIMIT", RESULT_RESOURCE_LIMIT),
        ("RV_RESULT_NO_CONTEXT", RESULT_NO_CONTEXT),
        ("RV_RESULT_GL_FAILURE", RESULT_GL_FAILURE),
        ("RV_RESULT_CLOSING", RESULT_CLOSING),
        ("RV_RESULT_UNSUPPORTED_OPERATION", RESULT_UNSUPPORTED_OPERATION),
        ("RV_RESULT_INTERNAL_ERROR", RESULT_INTERNAL_ERROR),
        ("RV_FEATURE_SURFACE_COMMANDS", FEATURE_SURFACE_COMMANDS),
        ("RV_FEATURE_COMPLETED_FRAMES", FEATURE_COMPLETED_FRAMES),
        ("RV_FEATURE_SCALE2X", FEATURE_SCALE2X),
        ("RV_FEATURE_ACTUAL_DEVICE", FEATURE_ACTUAL_DEVICE),
        ("RV_FEATURE_SURFACE_SCALE", FEATURE_SURFACE_SCALE),
    ],
)
def test_result_and_feature_values_match_native_header(header_name, python_value):
    assert _header_constant(header_name) == python_value


def test_independent_valid_wire_fixture_parses():
    fixture = _fixtures()["valid_clear_frame"]
    result = validate_packet(bytes.fromhex(fixture["hex"]), expected_epoch=fixture["epoch"])
    assert result.header.total_bytes == 40
    assert result.header.packet_sequence == 9
    assert [(command.opcode, command.command_bytes, command.offset) for command in result.commands] == [
        (OP_CLEAR_FRAME, 8, PACKET_HEADER_BYTES)
    ]


def test_zero_packet_sequence_is_rejected():
    packet = bytearray.fromhex(_fixtures()["valid_clear_frame"]["hex"])
    packet[24:32] = bytes(8)
    with pytest.raises(ProtocolError, match="packet sequence"):
        validate_packet(packet, expected_epoch=7)


@pytest.mark.parametrize(
    ("fixture_name", "epoch"),
    [
        ("stale_epoch", 7),
        ("truncated", 7),
        ("command_size_overflow", 7),
        ("reserved_flags", 7),
        ("unknown_opcode", 7),
        ("extra_bytes", 7),
    ],
)
def test_invalid_independent_wire_fixtures_are_rejected(fixture_name, epoch):
    packet = bytes.fromhex(_fixtures()[fixture_name]["hex"])
    with pytest.raises(ProtocolError):
        validate_packet(packet, expected_epoch=epoch)


@pytest.mark.parametrize(
    "changes",
    [
        {"backend": "future"},
        {"detail_scale": 3},
        {"aspect_num": 0},
        {"aspect_den": MAX_ASPECT_COMPONENT + 1},
        {"fit": "stretch"},
        {"texture_filter": "ai"},
        {"gpu_preference": "guess"},
    ],
)
def test_graphics_policy_rejects_unsupported_values(changes):
    values = dict(backend="legacy", detail_scale=2, aspect_num=4, aspect_den=3,
                  fit="desktop-height-preserve-aspect", texture_filter="nearest",
                  gpu_preference="auto")
    values.update(changes)
    with pytest.raises(ValueError):
        GraphicsPolicy(**values)


def test_graphics_policy_is_immutable_and_accepts_explicit_values():
    policy = GraphicsPolicy("common-v1", 4, 16, 9, "desktop-height-preserve-aspect", "linear", "dedicated")
    with pytest.raises((AttributeError, TypeError)):
        policy.detail_scale = 2


def test_capabilities_require_a_real_build_id_and_boolean_flags():
    caps = GraphicsCapabilities(1, BUILD_ID, True, False, True, True)
    assert caps.abi_version == 1
    with pytest.raises(ValueError, match="build_id"):
        GraphicsCapabilities(1, "not-a-build-id", True, False, True, True)
    with pytest.raises(ValueError, match="must be a bool"):
        GraphicsCapabilities(1, BUILD_ID, 1, False, True, True)


def _frame(**changes):
    values = dict(struct_bytes=FRAME_DESC_BYTES, abi_version=ABI_VERSION,
                  frame_sequence=1, physical_width=512, physical_height=480,
                  physical_stride_bytes=2048, logical_width=256, logical_height=240,
                  source_scale=2, desired_scale=2, pixel_format=1, alpha_mode=2, origin=1)
    values.update(changes)
    return FrameDescriptor(**values)


def test_frame_bounds_accept_valid_scaled_frame_and_clear_overlay():
    assert validate_frame_payload(_frame(), 2048 * 480) == 2048 * 480
    empty = _frame(physical_width=0, physical_height=0, physical_stride_bytes=0,
                   logical_width=0, logical_height=0)
    assert validate_frame_payload(empty, 0, allow_empty=True) == 0


@pytest.mark.parametrize(
    ("desc", "pixel_bytes"),
    [
        (_frame(struct_bytes=FRAME_DESC_BYTES - 4), 2048 * 480),
        (_frame(abi_version=2), 2048 * 480),
        (_frame(physical_stride_bytes=100), 100 * 480),
        (_frame(physical_width=513), 2048 * 480),
        (_frame(logical_width=4097, logical_height=1025, physical_width=8194,
                physical_height=2050, physical_stride_bytes=32776), 32776 * 2050),
        (_frame(physical_stride_bytes=0xFFFFFFFF), 0xFFFFFFFF),
        (_frame(pixel_format=99), 2048 * 480),
        (_frame(origin=2), 2048 * 480),
    ],
)
def test_frame_bounds_reject_bad_stride_density_size_and_formats(desc, pixel_bytes):
    with pytest.raises(ProtocolError):
        validate_frame_payload(desc, pixel_bytes)


def test_frame_payload_rejects_u32_multiplication_overflow_before_accepting_size():
    huge = _frame(physical_width=0x40000000, physical_height=4,
                  physical_stride_bytes=0xFFFFFFFF, logical_width=0x40000000,
                  logical_height=4, source_scale=1)
    with pytest.raises(ProtocolError, match="overflow|limit"):
        validate_frame_payload(huge, 0xFFFFFFFF)
