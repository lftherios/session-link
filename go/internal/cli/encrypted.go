package cli

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/lftherios/session-link/internal/format"
	"github.com/lftherios/session-link/internal/identity"
	"github.com/lftherios/session-link/internal/scan"
	"github.com/lftherios/session-link/internal/sealed"
)

var shareID = regexp.MustCompile(`^[23456789abcdefghjkmnpqrstuvwxyz]{14}$`)

type shareReceipt struct {
	Account  string `json:"account,omitempty"`
	Server   string `json:"server"`
	Key      string `json:"key"`
	SHA256   string `json:"sha256"`
	URL      string `json:"url,omitempty"`
	Envelope []byte `json:"envelope,omitempty"` // Pending only, for retry after a lost acknowledgment.
}

func saveReceipt(file string, receipt shareReceipt) error {
	dir := filepath.Dir(file)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	if err := os.Chmod(dir, 0o700); err != nil {
		return err
	}
	f, err := os.CreateTemp(dir, ".receipt-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if err = json.NewEncoder(f).Encode(receipt); err == nil {
		err = f.Sync()
	}
	closeErr := f.Close()
	if err != nil {
		return err
	}
	if closeErr != nil {
		return closeErr
	}
	if err = os.Rename(f.Name(), file); err != nil {
		return err
	}
	d, err := os.Open(dir)
	if err != nil {
		return err
	}
	defer d.Close()
	return d.Sync()
}

// UploadRun validates and scans the exact export, then uploads only ciphertext.
// Never fall back to the plaintext endpoint on an older or unavailable server.
// The pending receipt is durable before upload; retrying after an interrupted
// response resends the same ciphertext and recovers the same share URL.
func UploadRun(text, server, apiKey string) UploadResult {
	fail := func(code, message string) UploadResult { return UploadResult{Body: errBody(code, message)} }
	u, err := url.Parse(server)
	if err != nil || u == nil || u.Host == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" {
		return fail("invalid_server", "invalid server URL")
	}
	loopback := u.Hostname() == "localhost" || net.ParseIP(u.Hostname()).IsLoopback()
	if u.Scheme != "https" && !(u.Scheme == "http" && loopback) {
		return fail("invalid_server", "use HTTPS for the server (HTTP is allowed only on localhost)")
	}
	server = strings.TrimRight(server, "/")
	if apiKey == "" {
		return UploadResult{Status: 401, Body: errBody("auth_required", "run slink login before publishing")}
	}
	if len(text) > sealed.MaxPlaintext {
		return UploadResult{Status: 413, Body: errBody("too_large", "session exceeds the 25 MiB limit")}
	}
	var doc any
	if json.Unmarshal([]byte(text), &doc) != nil || len(format.ValidateRun(doc)) != 0 {
		return UploadResult{Status: 422, Body: errBody("invalid_session", "not a valid session document")}
	}
	if len(scan.ForSecrets(text)) != 0 {
		return UploadResult{Status: 422, Body: errBody("secrets_detected", "redact credentials before publishing")}
	}
	pendingID := sha256.Sum256([]byte(server + "\x00" + text))
	pendingPrefix := filepath.Join(Home(), "pending-shares", hex.EncodeToString(pendingID[:]))
	pendingFile := pendingPrefix + ".json"
	if matches, _ := filepath.Glob(pendingPrefix + "-*.json"); len(matches) > 0 {
		pendingFile = matches[0]
	}
	var receipt shareReceipt
	if saved, readErr := os.ReadFile(pendingFile); readErr == nil {
		if json.Unmarshal(saved, &receipt) != nil {
			return fail("local_keys", "cannot read the pending encryption receipt")
		}
		plain, decryptErr := sealed.Decrypt(receipt.Envelope, receipt.Key)
		if decryptErr != nil || receipt.Server != server || !bytes.Equal(plain, []byte(text)) {
			return fail("local_keys", "pending encryption receipt failed verification")
		}
	} else if !os.IsNotExist(readErr) {
		return fail("local_keys", "cannot read local encryption keys")
	} else {
		data, key, sealErr := sealed.Encrypt([]byte(text))
		if sealErr != nil {
			return fail("encryption_failed", sealErr.Error())
		}
		hash := sha256.Sum256(data)
		receipt = shareReceipt{Server: server, Key: key, SHA256: hex.EncodeToString(hash[:]), Envelope: data}
		pendingFile = pendingPrefix + "-" + receipt.SHA256 + ".json"
		if err := saveReceipt(pendingFile, receipt); err != nil {
			return fail("local_keys", "cannot save the encryption key; nothing was uploaded")
		}
	}
	req, err := http.NewRequest(http.MethodPost, server+"/api/shares", bytes.NewReader(receipt.Envelope))
	if err != nil {
		return fail("invalid_server", err.Error())
	}
	req.Header.Set("content-type", "application/vnd.session-link.encrypted")
	req.Header.Set("authorization", "Bearer "+apiKey)
	client := &http.Client{Timeout: 5 * time.Minute, CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse }}
	res, err := client.Do(req)
	if err != nil {
		return fail("unreachable", "upload was not acknowledged; retry to recover it with the saved local key")
	}
	defer res.Body.Close()
	var out map[string]any
	decodeErr := json.NewDecoder(io.LimitReader(res.Body, 64*1024)).Decode(&out)
	if res.StatusCode < 200 || res.StatusCode >= 300 {
		if out == nil {
			out = errBody("upload_failed", "encrypted upload failed; no plaintext fallback was attempted")
		}
		return UploadResult{Status: res.StatusCode, Body: out}
	}
	id, _ := out["id"].(string)
	if decodeErr != nil || !shareID.MatchString(id) || out["sha256"] != receipt.SHA256 {
		return fail("invalid_response", "server did not acknowledge the expected encrypted document; retry to recover the share")
	}
	// Construct the link on the configured origin. A response cannot redirect
	// the encryption key to a different site's JavaScript.
	receipt.URL = server + "/s/" + id + "#key=" + receipt.Key
	receipt.Envelope = nil
	receipt.Account, _ = out["account_id"].(string)
	if err := saveReceipt(filepath.Join(Home(), "shares", receipt.SHA256+"-"+id+".json"), receipt); err != nil {
		return fail("local_keys", "upload succeeded; its key remains in pending-shares. Retry to save the complete link")
	}
	_ = os.Remove(pendingFile)
	out["url"] = receipt.URL
	out["backup"] = (identity.Client{Home: Home(), Server: server, APIKey: apiKey}).Backup(context.Background(), receipt.Account)
	return UploadResult{OK: true, Status: res.StatusCode, Body: out}
}

// DeleteShare keeps the URL fragment entirely local.
func DeleteShare(server, apiKey, id string) (int, string) {
	if !shareID.MatchString(id) {
		return 0, "invalid share id"
	}
	req, err := http.NewRequest(http.MethodDelete, strings.TrimRight(server, "/")+"/api/shares/"+id, nil)
	if err != nil {
		return 0, err.Error()
	}
	req.Header.Set("authorization", "Bearer "+apiKey)
	res, err := (&http.Client{Timeout: 30 * time.Second}).Do(req)
	if err != nil {
		return 0, fmt.Sprint(err)
	}
	defer res.Body.Close()
	return res.StatusCode, http.StatusText(res.StatusCode)
}
