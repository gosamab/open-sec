// HTTP handler that serves a file from a fixed docs root, joined with an
// untrusted query parameter. filepath.Join cleans `..` to a canonical form
// but does NOT enforce containment within the base — `../../etc/passwd`
// still escapes.
// Vulnerable: CWE-22 (Path Traversal).

package main

import (
	"io"
	"net/http"
	"os"
	"path/filepath"
)

const docsRoot = "/var/app/docs"

func download(w http.ResponseWriter, r *http.Request) {
	name := r.URL.Query().Get("name")
	full := filepath.Join(docsRoot, name)

	f, err := os.Open(full)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	defer f.Close()
	io.Copy(w, f)
}

func main() {
	http.HandleFunc("/download", download)
	http.ListenAndServe(":3000", nil)
}
