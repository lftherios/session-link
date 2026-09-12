package open

import (
	"strings"
	"testing"
)

func TestNamedRevokeRequiresIdsAndNeverReportsUnperformedSuccess(t *testing.T) {
	t.Setenv("SLINK_HOME", t.TempDir())
	s := &Server{PreviewDir: t.TempDir(), Target: "http://127.0.0.1:1"}
	if w := composeRequest(s, "POST", "/api/named", map[string]string{"action": "status"}); w.Code != 200 || !strings.Contains(w.Body.String(), "\"shares\":[]") {
		t.Fatal("status listing without an outbox should stay local", w.Code, w.Body.String())
	}
	for _, body := range []map[string]string{{"action": "revoke"}, {"action": "revoke", "id": "23456789abcdef"}, {"action": "revoke", "invite_id": "invite"}} {
		if w := composeRequest(s, "POST", "/api/named", body); w.Code != 400 {
			t.Fatal("incomplete revocation accepted", body, w.Code)
		}
	}
	// The publish server is unreachable here, so a revocation cannot have
	// happened; the viewer must not be told that it did.
	if w := composeRequest(s, "POST", "/api/named", map[string]string{"action": "revoke", "id": "23456789abcdef", "invite_id": "invite"}); w.Code == 200 {
		t.Fatal("revocation that never reached the server reported success", w.Body.String())
	}
}
