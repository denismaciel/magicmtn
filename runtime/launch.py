#!/usr/bin/env python3
"""Launch sandbox commands with the same configuration as the managed agent."""
import os
import sys

import environment


def main():
    command = sys.argv[1:]
    if not command:
        raise SystemExit("a sandbox command is required")
    os.execvpe(command[0], command, environment.load())


if __name__ == "__main__":
    main()
