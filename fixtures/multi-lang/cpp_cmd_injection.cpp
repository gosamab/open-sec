// Tiny CGI-style binary that pings a host taken from argv.
// Vulnerable: CWE-78 (OS Command Injection) via system().

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

int main(int argc, char* argv[]) {
    if (argc < 2) {
        std::fprintf(stderr, "usage: %s <host>\n", argv[0]);
        return 2;
    }

    // argv[1] is fully attacker-controlled and concatenated into a string
    // that is then passed to /bin/sh. A payload like `127.0.0.1; cat /etc/passwd`
    // executes the appended command.
    std::string cmd = "ping -c 1 ";
    cmd += argv[1];

    int rc = std::system(cmd.c_str());
    return rc;
}
