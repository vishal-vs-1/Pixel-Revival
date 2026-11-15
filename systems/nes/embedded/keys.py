"""Translate control tokens into Mesen names and Tk key events."""
from __future__ import annotations




def _key_name(token: str) -> str:
    if token.startswith("KEY_"):
        return token[-1]
    if token.startswith("DIGIT_"):
        return token[-1]
    if token.startswith("NUMPAD_"):
        return "Numpad " + token[-1]
    return {
        "ARROW_UP": "Up Arrow", "ARROW_DOWN": "Down Arrow",
        "ARROW_LEFT": "Left Arrow", "ARROW_RIGHT": "Right Arrow",
        "ENTER": "Enter", "NUMPAD_ENTER": "Numpad Enter", "SPACE": "Space",
    }[token]


def _key_event_name(token: str) -> str:
    if token.startswith("KEY_"):
        return token[-1].lower()
    if token.startswith("DIGIT_"):
        return token[-1]
    if token.startswith("NUMPAD_"):
        return "KP_" + token[-1]
    return {
        "ARROW_UP": "up", "ARROW_DOWN": "down", "ARROW_LEFT": "left",
        "ARROW_RIGHT": "right", "ENTER": "return", "NUMPAD_ENTER": "kp_enter",
        "SPACE": "space",
    }[token]
