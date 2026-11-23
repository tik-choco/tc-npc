package server

import (
	"io/fs"
	"mime"
	"net/http"
	"path"
	"strings"

	"github.com/tik-choco/tc-npc/web"
)

func staticHandler(w http.ResponseWriter, r *http.Request) {
	name := strings.TrimLeft(r.URL.EscapedPath(), "/")
	if name == "" {
		name = "index.html"
	}
	data, err := fs.ReadFile(web.Dist, "dist/"+name)
	if err != nil {
		name = "index.html"
		data, err = fs.ReadFile(web.Dist, "dist/"+name)
	}
	if err != nil {
		writeText(w, http.StatusNotFound, "web/dist is empty — build the web frontend first")
		return
	}
	writeBytes(w, assetMIME(name), data)
}

func assetMIME(name string) string {
	ext := strings.ToLower(path.Ext(name))
	switch ext {
	case ".html", ".htm":
		return "text/html"
	case ".css":
		return "text/css"
	case ".mjs":
		return "application/javascript"
	case ".js":
		return "text/javascript"
	case ".ico":
		return "image/x-icon"
	case ".map":
		return "text/plain"
	case ".woff":
		return "application/font-woff"
	}
	mediaType, _, _ := mime.ParseMediaType(mime.TypeByExtension(ext))
	if mediaType == "" {
		return "application/octet-stream"
	}
	return mediaType
}
