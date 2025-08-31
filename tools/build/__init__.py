import argparse
import importlib
import sys

from typing import Callable, Dict, List

from .context import Context


class Command:
    def __init__(self, name: str, group: str,
                 func: Callable[[Context, argparse.Namespace, List[str]], None],
                 args: Dict[str, str]):
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
CUR_GROUP: str = None
COMMAND_TABLE: Dict[str, Command] = {}


def load_commands() -> Dict[str, Command]:
    global CUR_GROUP
    for name, desc in PLUGINS:
        CUR_GROUP = desc
        importlib.import_module(f"build.{name}")
    return COMMAND_TABLE


def command(name: str, args: List[Dict[str, str]] = None) -> Callable:
    """Decorator used inside plugins, e.g. @command('clean')."""
    def decorator(func: Callable) -> Callable:
        if name in COMMAND_TABLE:
            sys.exit(f"Command '{name}' does already exist.")
        COMMAND_TABLE[name] = Command(name, CUR_GROUP, func, args)
        return func
    return decorator
