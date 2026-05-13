# File-serving endpoint with no path containment.
# Expected finding: vuln, high, CWE-22, around lines 11-15.

import os
from flask import Flask, request, send_file, abort

app = Flask(__name__)
DOCS_ROOT = "/var/app/docs"


@app.route("/download")
def download():
    name = request.args.get("name", "")
    # Joins user input straight onto a root path. "../../etc/passwd" escapes.
    full_path = os.path.join(DOCS_ROOT, name)
    if not os.path.exists(full_path):
        abort(404)
    return send_file(full_path)


if __name__ == "__main__":
    app.run()
