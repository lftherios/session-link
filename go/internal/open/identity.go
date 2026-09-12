package open

import (
	"encoding/json"
	"net/http"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/identity"
)

func (s *Server) identityAPI(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("content-type", "application/json")
	if !localAction(r) {
		http.Error(w, `{"error":{"message":"cross-origin request blocked"}}`, 403)
		return
	}
	if r.Method != "POST" {
		http.Error(w, `{"error":{"message":"POST required"}}`, 405)
		return
	}
	var input struct {
		Action string `json:"action"`
		identity.Input
	}
	if json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096)).Decode(&input) != nil {
		http.Error(w, `{"error":{"message":"invalid request"}}`, 400)
		return
	}
	s.identityMu.Lock()
	defer s.identityMu.Unlock()
	client := identity.Client{Home: cli.Home(), Server: s.Target, APIKey: s.apiKey()}
	result, err := client.Handle(r.Context(), input.Action, input.Input)
	if err != nil {
		w.WriteHeader(identity.HTTPStatus(err))
		json.NewEncoder(w).Encode(map[string]any{"error": map[string]string{"message": err.Error()}})
		return
	}
	json.NewEncoder(w).Encode(result)
}
