"""OCHRE API compatibility layer."""

from ochre_next.compat.ochre_config import (
    migrate_ochre_equipment_config,
    ochre_generator_config,
)

__all__ = ["migrate_ochre_equipment_config", "ochre_generator_config"]
