"""Labels shared by launcher views."""

from systems.contracts import JAVA_ME, NES

GPU_CHOICES={'Auto · dedicated first':'auto','Dedicated GPU':'dedicated','Integrated GPU':'integrated','Windows default':'system'}
SYSTEM_LABELS={JAVA_ME:'Java ME',NES:'NES'}

ARTWORK_LABELS = {"pixel": "Fast pixel", "ai": "AI cartoon", "redraw": "Redraw (no outlines)"}
ARTWORK_BACKENDS = {label: backend for backend, label in ARTWORK_LABELS.items()}
