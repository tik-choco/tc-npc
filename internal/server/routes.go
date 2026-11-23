package server

import (
	"net/http"
	"net/url"
	"strings"
	"unicode/utf8"
)

type endpoint struct {
	method  string
	handler http.HandlerFunc
}

func (s *appState) routes() http.Handler {
	paths := http.NewServeMux()
	methods := http.NewServeMux()
	get, post, put, patch, del := http.MethodGet, http.MethodPost, http.MethodPut, http.MethodPatch, http.MethodDelete
	register := func(path string, endpoints ...endpoint) {
		_, parameter, _ := strings.Cut(path, "{")
		parameter, _, _ = strings.Cut(parameter, "}")
		var allowed []string
		for _, e := range endpoints {
			handler := e.handler
			if parameter != "" {
				handler = func(w http.ResponseWriter, r *http.Request) {
					value, err := url.PathUnescape(r.PathValue(parameter))
					if err != nil || !utf8.ValidString(value) {
						writeText(w, http.StatusBadRequest, "Invalid URL: Invalid UTF-8 in `"+parameter+"`")
						return
					}
					r.SetPathValue(parameter, value)
					e.handler(w, r)
				}
			}
			methods.HandleFunc(e.method+" "+path, handler)
			allowed = append(allowed, e.method)
			if e.method == get {
				allowed = append(allowed, http.MethodHead)
			}
		}
		// Axum picks the most specific path before checking the method.
		// In particular, POST/DELETE /api/vrm/default must not fall
		// through to /api/vrm/{file}.
		paths.HandleFunc(path, func(w http.ResponseWriter, r *http.Request) {
			for _, method := range allowed {
				if r.Method == method {
					methods.ServeHTTP(w, r)
					return
				}
			}
			w.Header().Set("Allow", strings.Join(allowed, ","))
			w.WriteHeader(http.StatusMethodNotAllowed)
		})
	}
	register("/healthz", endpoint{get, healthz})
	register("/ws", endpoint{get, s.wsHandler})
	register("/api/state", endpoint{get, s.apiState})
	register("/api/mist/sync", endpoint{get, s.apiMistSync})
	register("/api/mist/rooms", endpoint{get, s.apiMistRooms})
	register("/api/config", endpoint{get, s.apiGetConfig}, endpoint{put, s.apiPutConfig})
	register("/api/characters", endpoint{get, s.apiListCharacters})
	register("/api/characters/import", endpoint{post, s.apiImportCharacters})
	register("/api/characters/{id}/activate", endpoint{post, s.apiActivateCharacter})
	register("/api/characters/{id}/avatar", endpoint{put, s.apiSetCharacterAvatar})
	register("/api/vrm", endpoint{get, s.apiListVRM})
	register("/api/vrm/file/{file}", endpoint{get, s.apiGetVRMFile})
	register("/api/vrm/default", endpoint{put, s.apiSetDefaultAvatar})
	register("/api/vrm/{file}", endpoint{post, s.apiUploadVRM}, endpoint{del, s.apiDeleteVRM})
	register("/api/sprites", endpoint{get, s.apiListSprites})
	register("/api/sprites/file/{file}", endpoint{get, s.apiGetSpriteFile})
	register("/api/sprites/{file}", endpoint{post, s.apiUploadSprite}, endpoint{del, s.apiDeleteSprite})
	register("/api/sound", endpoint{get, s.apiListSound})
	register("/api/sound/reveal", endpoint{post, s.apiRevealSoundFolder})
	register("/api/scheduler/test", endpoint{post, s.apiSchedulerTest})
	register("/api/scheduler/export", endpoint{get, s.apiSchedulerExport})
	register("/api/scheduler/import", endpoint{post, s.apiSchedulerImport})
	register("/api/schedule-profiles", endpoint{get, s.apiListScheduleProfiles}, endpoint{post, s.apiSaveScheduleProfile})
	register("/api/schedule-profiles/{id}/activate", endpoint{post, s.apiActivateScheduleProfile})
	register("/api/schedule-profiles/{id}", endpoint{del, s.apiDeleteScheduleProfile})
	register("/api/memory", endpoint{get, s.apiMemory})
	register("/api/affect/history", endpoint{get, s.apiAffectHistory})
	register("/api/chat/history", endpoint{get, s.apiChatHistory})
	register("/api/people", endpoint{get, s.apiListPeople}, endpoint{post, s.apiCreatePerson})
	register("/api/people/{id}", endpoint{get, s.apiGetPerson}, endpoint{patch, s.apiUpdatePerson}, endpoint{del, s.apiDeletePerson})
	register("/api/audio/devices", endpoint{get, apiAudioDevices})
	register("/api/llm/ocr", endpoint{post, s.apiLLMOCR})
	register("/api/llm/transcribe/{filename}", endpoint{post, s.apiLLMTranscribe})
	register("/api/llm/models", endpoint{post, s.apiLLMModels})
	register("/api/llm/voices", endpoint{post, s.apiLLMVoices})
	fallback := func(w http.ResponseWriter, r *http.Request) {
		first, _, _ := strings.Cut(strings.TrimPrefix(r.URL.Path, "/"), "/")
		if first == "api" || first == "ws" {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		staticHandler(w, r)
	}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Match the URI's escaped path as Axum does, then decode only
		// captured segments. An escaped literal such as %64efault is
		// therefore a filename, even though /default is a fixed route.
		escaped := r.URL.EscapedPath()
		if strings.Contains(escaped, "//") {
			fallback(w, r)
			return
		}
		matchURL := *r.URL
		matchURL.Path, matchURL.RawPath = escaped, ""
		// Hide dot segments from ServeMux's redirects. The path validator
		// still receives them after dispatch.
		segments := strings.Split(matchURL.EscapedPath(), "/")
		for i, segment := range segments {
			if segment == "." || segment == ".." {
				segments[i] = strings.ReplaceAll(segment, ".", "%2e")
			}
		}
		matchURL.RawPath = strings.Join(segments, "/")
		match := r.WithContext(r.Context())
		match.URL = &matchURL
		handler, pattern := paths.Handler(match)
		if pattern == "" {
			fallback(w, r)
			return
		}
		handler.ServeHTTP(w, match)
	})
}
