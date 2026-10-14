"""Colours and the GTK CSS of the Klip window. Dark, flat, rounded: the brand's ink and lime."""

INK = "#0B0E10"
PANEL = "#12171B"
PANEL_HI = "#192027"
LINE = "#1F272D"
TEXT = "#E7ECEF"
DIM = "#8B979F"
FAINT = "#5C6870"
LIME = "#C8F25A"
LIME_SOFT = "rgba(200, 242, 90, 0.12)"
AMBER = "#F2B84B"
RED = "#FF6B6B"

STATE_COLOURS = {"connected": LIME, "waiting": AMBER, "no_devices": FAINT, "paused": LIME, "stopped": FAINT}

CSS = f"""
window.klip {{ background: transparent; }}
.klip-frame {{
  background: {INK}; border-radius: 18px; border: 1px solid {LINE};
  box-shadow: 0 14px 44px rgba(0, 0, 0, 0.55);
}}
.klip label {{ color: {TEXT}; }}
.klip .dim {{ color: {DIM}; }}
.klip .faint {{ color: {FAINT}; }}
.klip .title {{ font-size: 15px; font-weight: 700; }}
.klip .big {{ font-size: 18px; font-weight: 700; }}
.klip .small {{ font-size: 11.5px; }}
.klip .mono {{ font-family: monospace; font-size: 11.5px; }}
.klip .section {{ font-size: 11px; font-weight: 700; letter-spacing: 1px; color: {FAINT}; }}
.klip .ok {{ color: {LIME}; }}
.klip .warn {{ color: {AMBER}; }}
.klip .bad {{ color: {RED}; }}

.klip .header {{ padding: 12px 14px 8px 16px; }}
.klip .wordmark {{ font-size: 15px; font-weight: 800; letter-spacing: 0.5px; }}
.klip .pill {{ background: {PANEL}; border-radius: 99px; padding: 3px 11px; border: 1px solid {LINE}; }}
.klip .pill label {{ font-size: 11.5px; font-weight: 600; }}

.klip .card {{ background: {PANEL}; border-radius: 14px; border: 1px solid {LINE}; padding: 14px; }}
.klip .card.accent {{ border-color: rgba(200, 242, 90, 0.35); background: {LIME_SOFT}; }}
.klip .card.warnbox {{ border-color: rgba(242, 184, 75, 0.4); background: rgba(242, 184, 75, 0.08); }}

.klip button {{
  background: {PANEL}; color: {TEXT}; border: 1px solid {LINE}; border-radius: 99px;
  padding: 7px 16px; box-shadow: none; text-shadow: none; background-image: none; font-weight: 600;
}}
.klip button:hover {{ background: {PANEL_HI}; }}
.klip button:disabled {{ color: {FAINT}; background: {PANEL}; }}
.klip button.primary {{ background: {LIME}; color: {INK}; border-color: {LIME}; }}
.klip button.primary label {{ color: {INK}; font-weight: 700; }}
.klip button.primary:hover {{ background: #D6F77E; }}
.klip button.primary:disabled {{ background: #3A4A22; border-color: #3A4A22; }}
.klip button.primary:disabled label {{ color: #7C8B63; }}
.klip button.danger {{ color: {RED}; }}
.klip button.danger label {{ color: {RED}; }}
.klip button.flat {{ background: transparent; border-color: transparent; }}
.klip button.flat:hover {{ background: {PANEL}; }}
.klip button.icon {{ padding: 6px; min-width: 28px; min-height: 28px; }}

.klip entry, .klip textview, .klip textview text {{
  background: {PANEL}; color: {TEXT}; border-radius: 12px; caret-color: {LIME};
}}
.klip entry {{ border: 1px solid {LINE}; padding: 8px 12px; box-shadow: none; }}
.klip entry:focus {{ border-color: {LIME}; }}
.klip .editor {{ background: {PANEL}; border: 1px solid {LINE}; border-radius: 12px; padding: 6px; }}
.klip .editor:focus-within {{ border-color: {LIME}; }}
.klip .editor textview {{ background: transparent; }}
.klip .editor textview text {{ background: transparent; }}
.klip .editor.mono textview {{ font-family: monospace; font-size: 11.5px; }}

.klip switch {{ background: {PANEL_HI}; border-radius: 99px; border: 1px solid {LINE}; min-height: 22px; min-width: 40px; }}
.klip switch slider {{ background: {DIM}; border-radius: 99px; border: none; min-width: 18px; min-height: 18px; margin: 1px; }}
.klip switch:checked {{ background: {LIME}; border-color: {LIME}; }}
.klip switch:checked slider {{ background: {INK}; }}

.klip scrolledwindow {{ background: transparent; }}
.klip scrollbar {{ background: transparent; }}
.klip scrollbar slider {{ background: {LINE}; border-radius: 99px; min-width: 5px; min-height: 5px; }}
.klip list, .klip list row {{ background: transparent; }}
.klip list row {{ border-radius: 12px; padding: 0; }}
.klip list row:hover {{ background: {PANEL}; }}
.klip list row:selected {{ background: {PANEL_HI}; }}

.klip .nav {{ background: {PANEL}; border-top: 1px solid {LINE}; border-bottom-left-radius: 18px; border-bottom-right-radius: 18px; padding: 6px 8px; }}
.klip .nav button {{ background: transparent; border: none; border-radius: 12px; padding: 6px 4px; }}
.klip .nav button label {{ font-size: 10.5px; font-weight: 600; color: {DIM}; }}
.klip .nav button image {{ color: {DIM}; }}
.klip .nav button:hover {{ background: {PANEL_HI}; }}
.klip .nav button.current label, .klip .nav button.current image {{ color: {LIME}; }}

.klip .toast {{ background: {TEXT}; border-radius: 99px; padding: 8px 16px; }}
.klip .toast label {{ color: {INK}; font-weight: 600; }}
.klip .toast.bad {{ background: {RED}; }}
.klip .sheet {{ background: {PANEL_HI}; border-top-left-radius: 16px; border-top-right-radius: 16px; border: 1px solid {LINE}; padding: 16px; }}
.klip .thumb {{ background: {INK}; border-radius: 10px; border: 1px solid {LINE}; }}
.klip .qr {{ background: #FFFFFF; border-radius: 14px; padding: 10px; }}
.klip separator {{ background: {LINE}; min-height: 1px; }}
popover.klip, popover.klip.background {{ background: {PANEL_HI}; border-radius: 12px; border: 1px solid {LINE}; padding: 6px; }}
popover.klip button {{ border-radius: 8px; }}
popover.klip label {{ color: {TEXT}; }}
.klip menu, .klip .menu {{ background: {PANEL_HI}; color: {TEXT}; }}
"""
