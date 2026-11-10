"""Launcher color roles shared by native Tk controls and custom buttons."""

from tkinter import Canvas, Entry, Frame, Label


# Keep the Java ME swatches as the source palette so switching is reversible.
NES_SWATCHES = {
    '#0c121c': '#080808', '#121c29': '#141414', '#192636': '#202020',
    '#243243': '#303030', '#263649': '#303030', '#2a3d50': '#343434',
    '#354b60': '#454545', '#4a6378': '#626262', '#617f98': '#858585',
    '#1b3339': '#292929', '#254b49': '#414141',
    '#6e879e': '#a2a2a2', '#7994ac': '#b0b0b0', '#8ea5b9': '#bdbdbd',
    '#91b2c6': '#c9c9c9', '#9db7cc': '#cecece', '#b7c9d9': '#dadada',
    '#dce7f5': '#e8e8e8', '#e8f1fc': '#f4f4f4', '#eff5fc': '#ffffff',
    '#84e4c7': '#f5f5f5', '#a8f5df': '#ffffff', '#9cf0d5': '#f5f5f5',
    '#c4ffed': '#ffffff', '#102720': '#111111',
    '#ffb6b6': '#dedede', '#ffd0d0': '#ffffff',
}


def java_color(swatch):
    return swatch


def nes_color(swatch):
    return NES_SWATCHES.get(swatch, swatch)


def capture_widget_colors(parent):
    """Remember explicit launcher colors before either theme is applied."""
    widgets = []
    for child in parent.winfo_children():
        if isinstance(child, (Canvas, Entry, Frame, Label)):
            options = {
                name: child.cget(name)
                for name in ('background', 'foreground', 'insertbackground',
                             'selectbackground', 'selectforeground')
                if name in child.keys() and child.cget(name) in NES_SWATCHES
            }
            if options:
                widgets.append((child, options))
        widgets.extend(capture_widget_colors(child))
    return widgets
