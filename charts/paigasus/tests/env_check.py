# SPDX-License-Identifier: Apache-2.0
"""Assert every required console env key actually reaches the container.

Reads a rendered manifest stream on stdin and the required key list from $REQUIRED.
Prints one OK line, or the problems joined by "|". Lives in its own file rather than
inside the shell script because the checks need both quote styles.
"""

import os
import sys

import yaml


def main() -> int:
    required = set(os.environ["REQUIRED"].split())
    docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
    configmaps = {
        d["metadata"]["name"]: set(d.get("data", {}))
        for d in docs
        if d["kind"] == "ConfigMap"
    }

    problems = []
    consoles = 0

    for doc in docs:
        if doc["kind"] != "Deployment" or "console" not in doc["metadata"]["name"]:
            continue
        consoles += 1
        name = doc["metadata"]["name"]
        container = doc["spec"]["template"]["spec"]["containers"][0]

        # A key is only delivered if its NAME is a legal env var name. The kubelet drops the
        # rest silently, so a key that merely appears in the YAML is not evidence it arrives.
        seen = {e["name"] for e in container.get("env", [])}

        for source in container.get("envFrom", []):
            if "configMapRef" in source:
                seen |= configmaps.get(source["configMapRef"]["name"], set())
            elif "secretRef" in source:
                problems.append(
                    name + " uses envFrom.secretRef: Secret KEYS become env var NAMES, and a "
                    "hyphenated key is skipped without error. Map each one with secretKeyRef."
                )

        missing = sorted(required - seen)
        if missing:
            problems.append(name + " never receives: " + ", ".join(missing))

        illegal = sorted(
            n for n in seen if not n.replace("_", "").isalnum() or n[:1].isdigit()
        )
        if illegal:
            problems.append(name + " has illegal env var name(s): " + ", ".join(illegal))

    if consoles == 0:
        problems.append("no console Deployment rendered")

    if problems:
        print("|".join(problems))
    else:
        print("OK {0} console(s), all {1} keys delivered".format(consoles, len(required)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
