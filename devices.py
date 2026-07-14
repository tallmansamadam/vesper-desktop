"""
devices.py - Multi-vendor light controller.

Aggregates one or more provider backends (LIFX, WiZ, Hue, ...) behind the same
method surface the app already used for LifxLan, so scenes, effects, knobs, and
screen-sync work across brands unchanged. Each discovered light carries a
`.provider`; every command is delegated to it.

Adding a new brand = write a provider exposing discover / set_power / set_color /
set_brightness / set_white / stream_color / refresh / close, and append it here.
"""

import lifx_lan
import wiz

try:
    import hue
except Exception:                            # noqa: BLE001 - optional
    hue = None


class MultiController:
    def __init__(self):
        self.lifx = lifx_lan.LifxLan()
        self.wiz = wiz.WizProvider()
        self.hue = hue.HueProvider.from_settings() if hue else None
        self._providers = [self.lifx, self.wiz]
        if self.hue:
            self._providers.append(self.hue)

    # -- discovery + delegation -------------------------------------------

    def discover(self, **_kw):
        lights = []
        for p in self._providers:
            try:
                lights += p.discover()
            except Exception:                # noqa: BLE001 - one brand down != all
                pass
        lights.sort(key=lambda l: (getattr(l, "vendor", ""), l.label.lower()))
        return lights

    def set_power(self, light, *a, **kw):
        light.provider.set_power(light, *a, **kw)

    def set_color(self, light, *a, **kw):
        light.provider.set_color(light, *a, **kw)

    def set_brightness(self, light, *a, **kw):
        light.provider.set_brightness(light, *a, **kw)

    def set_white(self, light, *a, **kw):
        light.provider.set_white(light, *a, **kw)

    def stream_color(self, light, *a, **kw):
        light.provider.stream_color(light, *a, **kw)

    def refresh(self, light):
        light.provider.refresh(light)

    # -- Hue pairing (press the bridge link button) -----------------------

    def hue_available(self):
        return hue is not None

    def add_hue(self, ip=None, on_wait=None):
        if hue is None:
            raise RuntimeError("Hue support unavailable.")
        prov = self.hue or hue.HueProvider()
        if prov.pair(ip=ip, on_wait=on_wait):
            self.hue = prov
            if prov not in self._providers:
                self._providers.append(prov)
            return True
        return False

    def close(self):
        for p in self._providers:
            try:
                p.close()
            except Exception:                # noqa: BLE001
                pass
