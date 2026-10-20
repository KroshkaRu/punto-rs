#!/usr/bin/env python3
"""Render checked-in Material SVGs; build-time only (python3-cairo, gir1.2-rsvg-2.0)."""
from pathlib import Path
import xml.etree.ElementTree as ET
import cairo
import gi
gi.require_version("Rsvg", "2.0")
from gi.repository import Rsvg

root = Path(__file__).resolve().parents[1]
assets = root / "assets/material-icons"
namespace = "{http://www.w3.org/2000/svg}"
ET.register_namespace("", namespace[1:-1])
svg = ET.Element(namespace + "svg", {"width":"64", "height":"64", "viewBox":"0 0 64 64"})
ET.SubElement(svg, namespace + "rect", {"width":"64", "height":"64", "rx":"12", "fill":"#2c78be"})
group = ET.SubElement(svg, namespace + "g", {"transform":"translate(8 8) scale(2)", "fill":"#fff"})
group.extend(ET.parse(assets / "translate.svg").getroot())
(root / "packaging/punto-rs.svg").write_bytes(ET.tostring(svg))

def render(data, size, output):
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, size, size)
    rect = Rsvg.Rectangle()
    rect.x, rect.y, rect.width, rect.height = 0, 0, size, size
    Rsvg.Handle.new_from_data(data).render_document(cairo.Context(surface), rect)
    surface.write_to_png(str(output))
    return surface

render(ET.tostring(svg), 128, assets / "app.png")
for language in ["en", "ru"]:
    for active in [True, False]:
        name = f"punto-rs-{language}" + ("" if active else "-paused")
        tray = ET.Element(namespace + "svg", {"width":"64", "height":"64", "viewBox":"0 0 64 64"})
        ET.SubElement(tray, namespace + "rect", {"width":"64", "height":"64", "rx":"12", "fill":"#2c78be" if active else "#646464"})
        symbol = ET.SubElement(tray, namespace + "g", {"transform":"translate(12 2) scale(1.65)", "fill":"#fff"})
        symbol.extend(ET.parse(assets / "translate.svg").getroot())
        ET.SubElement(tray, namespace + "text", {"x":"32", "y":"59", "text-anchor":"middle", "font-family":"DejaVu Sans, sans-serif", "font-size":"18", "font-weight":"bold", "fill":"#fff"}).text = language.upper()
        (assets / f"{name}.svg").write_bytes(ET.tostring(tray))
        render(ET.tostring(tray), 64, assets / f"{name}.png")
for name in ["translate", "keyboard", "settings", "undo", "swap_horiz"]:
    text = (assets / f"{name}.svg").read_text().replace("<svg ", '<svg fill="#fff" ', 1)
    render(text.encode(), 32, assets / f"{name}.png")
