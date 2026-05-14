// Spring Boot endpoint that fetches an arbitrary URL on behalf of the caller.
// No allowlist, no internal-network blocking — attacker can hit
// http://169.254.169.254/ for cloud metadata or http://localhost services.
// Vulnerable: CWE-918 (SSRF).

package com.example.preview;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;

@RestController
public class PreviewController {

    @GetMapping("/preview")
    public String preview(@RequestParam String url) throws Exception {
        // `url` is user-controlled and used directly. Attacker can supply
        // http://169.254.169.254/latest/meta-data/ to read cloud creds,
        // http://localhost:6379/ to probe internal Redis, file:// URIs, etc.
        URL u = new URL(url);
        HttpURLConnection conn = (HttpURLConnection) u.openConnection();
        conn.setRequestMethod("GET");

        StringBuilder out = new StringBuilder();
        try (BufferedReader br = new BufferedReader(
                new InputStreamReader(conn.getInputStream(), StandardCharsets.UTF_8))) {
            String line;
            while ((line = br.readLine()) != null) out.append(line).append('\n');
        }
        return out.toString();
    }
}
