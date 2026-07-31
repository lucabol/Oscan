"""Shared extraction and expansion of compiler builtin documentation metadata."""

from dataclasses import dataclass
from pathlib import Path
import re


@dataclass(frozen=True)
class BuiltinRecord:
    category: str
    name: str
    signature: str
    description: str


BUILTIN_RE = re.compile(
    r'// @builtin'
    r'\s+category="(?P<cat>[^"]+)"'
    r'\s+name="(?P<name>[^"]+)"'
    r'\s+sig="(?P<sig>[^"]+)"'
    r'\s+desc="(?P<desc>[^"]+)"'
)

FAMILY_RE = re.compile(
    r'// @builtin-family'
    r'\s+category="(?P<cat>[^"]+)"'
    r'\s+name="(?P<name>[^"]+)"'
    r'\s+sig="(?P<sig>[^"]+)"'
    r'\s+desc="(?P<desc>[^"]+)"'
    r'\s+types="(?P<types>[^"]+)"'
)

MAP_FAMILY_RE = re.compile(
    r'// @builtin-map-family'
    r'\s+category="(?P<cat>[^"]+)"'
    r'\s+name="(?P<name>[^"]+)"'
    r'\s+sig="(?P<sig>[^"]+)"'
    r'\s+desc="(?P<desc>[^"]+)"'
    r'\s+sources="(?P<sources>[^"]+)"'
    r'\s+targets="(?P<targets>[^"]+)"'
)


def _split_types(value: str) -> list[str]:
    return [item.strip() for item in value.split(",") if item.strip()]


def extract_builtin_records(source_paths: list[Path]) -> list[BuiltinRecord]:
    records: list[BuiltinRecord] = []

    for source_path in source_paths:
        text = source_path.read_text(encoding="utf-8")

        for match in BUILTIN_RE.finditer(text):
            records.append(
                BuiltinRecord(
                    match.group("cat"),
                    match.group("name"),
                    match.group("sig"),
                    match.group("desc"),
                )
            )

        for match in FAMILY_RE.finditer(text):
            for type_name in _split_types(match.group("types")):
                values = {"type": type_name}
                records.append(
                    BuiltinRecord(
                        match.group("cat"),
                        match.group("name").format(**values),
                        match.group("sig").format(**values),
                        match.group("desc").format(**values),
                    )
                )

        for match in MAP_FAMILY_RE.finditer(text):
            for source in _split_types(match.group("sources")):
                for target in _split_types(match.group("targets")):
                    values = {"source": source, "target": target}
                    records.append(
                        BuiltinRecord(
                            match.group("cat"),
                            match.group("name").format(**values),
                            match.group("sig").format(**values),
                            match.group("desc").format(**values),
                        )
                    )

    names: set[str] = set()
    for record in records:
        if record.name in names:
            raise ValueError(f"duplicate builtin documentation metadata: {record.name}")
        names.add(record.name)

    return records
