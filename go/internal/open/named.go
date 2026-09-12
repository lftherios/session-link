package open

import (
	"context"
	"encoding/json"
	"net/http"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/identity"
)

func (s *Server) namedAPI(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("content-type", "application/json")
	if r.Method != "POST" || !localAction(r) {
		http.Error(w, `{"error":{"message":"local action required"}}`, 403)
		return
	}
	var in struct {
		Action     string `json:"action"`
		ID         string `json:"id"`
		Invitation string `json:"invite_id"`
	}
	if json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096)).Decode(&in) != nil {
		http.Error(w, `{"error":{"message":"invalid request"}}`, 400)
		return
	}
	if in.Action != "status" && in.Action != "revoke" {
		http.Error(w, `{"error":{"message":"unknown sharing action"}}`, 400)
		return
	}
	revoke := ""
	if in.Action == "revoke" {
		revoke = in.ID
	}
	result, err := (identity.Client{Home: cli.Home(), Server: s.Target, APIKey: s.apiKey()}).NamedShares(r.Context(), revoke, in.Invitation, true)
	if err != nil {
		w.WriteHeader(identity.HTTPStatus(err))
		json.NewEncoder(w).Encode(map[string]any{"error": map[string]string{"message": err.Error()}})
		return
	}
	json.NewEncoder(w).Encode(map[string]any{"shares": result})
}

// Pending recipient bindings resume whenever the native viewer is running.
// Only an invitation already authorized by the sender can release a key.
func (s *Server) resumeNamed(ctx context.Context) {
	timer := time.NewTicker(15 * time.Second)
	defer timer.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-timer.C:
		}
		_, _ = (identity.Client{Home: cli.Home(), Server: s.Target, APIKey: s.apiKey()}).NamedShares(ctx, "", "", false)
	}
}
