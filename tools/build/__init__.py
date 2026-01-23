import argparse
import importlib
import sys

from typing import Any, Callable, Dict, List, Optional, TypeVar, Union

from .context import Context

# tell b about the exported types (for mypy)
__all__ = ["Context", "load_commands"]


class Command:
    def __init__(self, name: str, group: str,
                 func: Callable[[Context, argparse.Namespace, List[str]], None],
                 args: Optional[List[Dict[str, Union[str, List[str]]]]]):
        self.name = name
        self.group = group
        self.func = func
        self.args = args


PLUGINS = [
    ("build",       "Building"),
    ("run",         "Running"),
    ("debug",       "Debugging"),
    ("analysis",    "Program analysis"),
    ("fs",          "File system"),
    ("maintenance", "Maintenance"),
    ("m3lx",        "M³Linux"),
]
CUR_GROUP: str = ""
COMMAND_TABLE: Dict[str, Command] = {}


def load_commands() -> Dict[str, Command]:
    global CUR_GROUP
    for name, desc in PLUGINS:
        CUR_GROUP = desc
        importlib.import_module(f"build.{name}")
    return COMMAND_TABLE


# don't be more specific here as this doesn't work with 3.9 that easily as it seems
F = TypeVar("F", bound=Callable[..., Any])


def command(
    name: str,
    args: Optional[List[Dict[str, Union[str, List[str]]]]] = None,
) -> Callable[[F], F]:
    """Decorator used inside plugins, e.g. @command('clean')."""
    def decorator(func: F) -> F:
        if name in COMMAND_TABLE:
            sys.exit(f"Command '{name}' does already exist.")
        COMMAND_TABLE[name] = Command(name, CUR_GROUP, func, args)
        return func
    return decorator
