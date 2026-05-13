# Flask endpoint that shells out with user-controlled input.
# Expected finding: vuln, critical/high, CWE-78, around lines 13-16.

from flask import Flask, request, jsonify
import subprocess

app = Flask(__name__)


@app.route("/ping")
def ping():
    host = request.args.get("host", "")
    # shell=True + string concat = command injection.
    result = subprocess.run(
        f"ping -c 1 {host}",
        shell=True,
        capture_output=True,
        text=True,
    )
    return jsonify({"stdout": result.stdout, "stderr": result.stderr})


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=5000)
