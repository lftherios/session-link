package identity

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/lftherios/session-link/internal/format"
	"github.com/lftherios/session-link/internal/scan"
	"github.com/lftherios/session-link/internal/sealed"
)

var namedID = regexp.MustCompile(`^[23456789abcdefghjkmnpqrstuvwxyz]{14}$`)
var emailAddress = regexp.MustCompile("^[a-z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?\\.[a-z]{2,63}$")

type Invitation struct {
	ID         string `json:"id"`
	Email      string `json:"email"`
	Commitment string `json:"commitment"`
}
type policy struct {
	Account    string       `json:"account"`
	Root       string       `json:"root"`
	Head       string       `json:"head"`
	Signer     string       `json:"signer"`
	SHA256     string       `json:"sha256"`
	Inbox      string       `json:"inbox"`
	Epoch      int          `json:"epoch"`
	Wrap       string       `json:"wrap"`
	Recipients []Invitation `json:"recipients"`
}
type claim struct {
	Account   string `json:"account"`
	Root      string `json:"root"`
	Head      string `json:"head"`
	Signer    string `json:"signer"`
	ShareID   string `json:"share_id"`
	InviteID  string `json:"invite_id"`
	SHA256    string `json:"sha256"`
	Owner     string `json:"owner"`
	OwnerRoot string `json:"owner_root"`
	Inbox     string `json:"inbox"`
	Epoch     int    `json:"epoch"`
}
type recipientClaim struct {
	Signed
	Proof string `json:"proof"`
}
type namedGrant struct {
	Account       string `json:"account"`
	Root          string `json:"root"`
	Head          string `json:"head"`
	Signer        string `json:"signer"`
	ShareID       string `json:"share_id"`
	InviteID      string `json:"invite_id"`
	SHA256        string `json:"sha256"`
	Recipient     string `json:"recipient"`
	RecipientRoot string `json:"recipient_root"`
	Inbox         string `json:"inbox"`
	Epoch         int    `json:"epoch"`
	Wrap          string `json:"wrap"`
	ClaimHash     string `json:"claim_hash"`
}
type outbox struct {
	Status      map[string]string `json:"status,omitempty"`
	Server      string            `json:"server"`
	Account     string            `json:"account"`
	Root        string            `json:"root"`
	ID          string            `json:"id"`
	SHA256      string            `json:"sha256"`
	Key         string            `json:"key"`
	Envelope    []byte            `json:"envelope,omitempty"`
	Invitations []Invitation      `json:"invitations"`
	Secrets     map[string]string `json:"secrets"`
}
type NamedRecipient struct {
	ID     string `json:"id"`
	Email  string `json:"email"`
	URL    string `json:"url"`
	Status string `json:"status"`
}
type NamedResult struct {
	ID         string           `json:"id"`
	URL        string           `json:"url"`
	Recipients []NamedRecipient `json:"recipients"`
	Error      string           `json:"error,omitempty"`
}
type namedRemote struct {
	ID         string `json:"id"`
	Owner      bool   `json:"owner"`
	SHA256     string `json:"sha256"`
	Recipients []struct {
		ID      string          `json:"id"`
		Email   string          `json:"email"`
		Status  string          `json:"status"`
		Claim   *recipientClaim `json:"claim"`
		History []Signed        `json:"recipient_history"`
	} `json:"recipients"`
}
type nativeKeys struct {
	remote remote
	state  State
	vault  Vault
	device *localDevice
	own    Device
	key    []byte
	data   contents
}

func (c Client) lockNative() (func(), error) {
	if err := os.MkdirAll(filepath.Join(c.Home, "identity"), 0700); err != nil {
		return nil, err
	}
	f, err := os.OpenFile(filepath.Join(c.Home, "identity", ".lock"), os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return nil, err
	}
	if err = lockDeviceFile(f); err != nil {
		f.Close()
		return nil, errors.New("another slink process is updating private keys; retry shortly")
	}
	return func() { f.Close() }, nil
}
func (c Client) nativeKeys(ctx context.Context, enable bool) (nativeKeys, error) {
	var out nativeKeys
	remote, err := c.request(ctx, nil)
	if err != nil {
		return out, err
	}
	if remote.Record == nil {
		return out, errors.New("set up recovery in Recovery and devices before sharing with people")
	}
	state, vault, err := remote.Record.verify(remote.Account)
	if err != nil {
		return out, err
	}
	d, err := c.load(remote.Account)
	if err != nil {
		return out, err
	}
	if d == nil {
		return out, ErrLocked
	}
	if err = c.pin(d, *remote.Record, state, vault); err != nil {
		return out, err
	}
	own, ok := state.device(d.public("").ID)
	if !ok {
		return out, ErrLocked
	}
	key, err := unwrap(d.Box, own.Wrap, state.context())
	if err != nil {
		return out, err
	}
	data, err := c.decrypt(key, state, vault)
	if err != nil {
		return out, err
	}
	if enable && state.Inbox == "" {
		if _, err = c.handle(ctx, "sharing", Input{}); err != nil {
			return out, err
		}
		return c.nativeKeys(ctx, false)
	}
	return nativeKeys{remote, state, vault, d, own, key, data}, nil
}
func (c Client) namedRequest(ctx context.Context, method, path string, headers map[string]string, body io.Reader, out any) error {
	req, err := http.NewRequestWithContext(ctx, method, c.Server+path, body)
	if err != nil {
		return err
	}
	req.Header.Set("authorization", "Bearer "+c.APIKey)
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	response, err := (&http.Client{Timeout: 5 * time.Minute, CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse }}).Do(req)
	if err != nil {
		return errors.New("cannot reach private sharing; your pending keys are saved locally")
	}
	defer response.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(response.Body, 12*1024*1024+1))
	if err != nil || len(raw) > 12*1024*1024 {
		return errors.New("invalid private-sharing response")
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		var fail struct {
			Error struct {
				Message string `json:"message"`
			} `json:"error"`
		}
		json.Unmarshal(raw, &fail)
		return &httpError{response.StatusCode, fmt.Sprintf("%s (HTTP %d)", fail.Error.Message, response.StatusCode)}
	}
	if out != nil && json.Unmarshal(raw, out) != nil {
		return errors.New("invalid private-sharing response")
	}
	return nil
}
func (c Client) namedPost(ctx context.Context, id string, body any, out any) error {
	raw, err := json.Marshal(body)
	if err != nil {
		return err
	}
	return c.namedRequest(ctx, "POST", "/api/named-shares/"+id, map[string]string{"content-type": "application/json"}, bytes.NewReader(raw), out)
}
func normalizedRecipients(values []string) ([]string, error) {
	if len(values) < 1 || len(values) > 10 {
		return nil, errors.New("choose between one and ten email recipients")
	}
	seen := map[string]bool{}
	for _, email := range values {
		email = strings.ToLower(strings.TrimSpace(email))
		if len(email) > 254 || !emailAddress.MatchString(email) {
			return nil, errors.New("enter a valid email address for each recipient")
		}
		seen[email] = true
	}
	out := []string{}
	for email := range seen {
		out = append(out, email)
	}
	sort.Strings(out)
	return out, nil
}
func namedContext(sha, account, root string, epoch int) string {
	return fmt.Sprintf("named/%s/%s/%s/%d", sha, account, root, epoch)
}
func (c Client) outboxFile(hash string) string {
	return filepath.Join(c.Home, "named-shares", hash+".json")
}
func (o outbox) result() NamedResult {
	out := NamedResult{ID: o.ID, URL: o.Server + "/n/" + o.ID, Recipients: []NamedRecipient{}}
	for _, recipient := range o.Invitations {
		out.Recipients = append(out.Recipients, NamedRecipient{ID: recipient.ID, Email: recipient.Email, URL: out.URL + "?invite=" + recipient.ID + "#invite=" + o.Secrets[recipient.ID] + "&sender=" + o.Root, Status: "waiting"})
		if status := o.Status[recipient.ID]; status != "" {
			out.Recipients[len(out.Recipients)-1].Status = status
			if status == "ready" {
				out.Recipients[len(out.Recipients)-1].URL = out.URL
			}
		}
	}
	return out
}
func (c Client) NamedPublish(ctx context.Context, text string, emails []string) (NamedResult, error) {
	operations.Lock()
	defer operations.Unlock()
	c.Server = strings.TrimRight(c.Server, "/")
	release, err := c.lockNative()
	if err != nil {
		return NamedResult{}, err
	}
	defer release()
	recipients, err := normalizedRecipients(emails)
	if err != nil {
		return NamedResult{}, err
	}
	var doc any
	if len(text) > sealed.MaxPlaintext || json.Unmarshal([]byte(text), &doc) != nil || len(format.ValidateRun(doc)) != 0 {
		return NamedResult{}, errors.New("not a valid supported session document")
	}
	if len(scan.ForSecrets(text)) != 0 {
		return NamedResult{}, errors.New("redact credentials before publishing")
	}
	keys, err := c.nativeKeys(ctx, true)
	if err != nil {
		return NamedResult{}, err
	}
	pending := c.outboxFile("pending-" + digest([]byte(c.Server+"\x00"+keys.state.Account+"\x00"+text+"\x00"+strings.Join(recipients, "\x00"))))
	var saved outbox
	if raw, readErr := os.ReadFile(pending); readErr == nil {
		if json.Unmarshal(raw, &saved) != nil {
			return NamedResult{}, errors.New("cannot read pending private share")
		}
		plain, e := sealed.Decrypt(saved.Envelope, saved.Key)
		if e != nil || string(plain) != text || saved.Server != c.Server || saved.Account != keys.state.Account || saved.Root != keys.state.Root {
			return NamedResult{}, errors.New("pending private share failed verification")
		}
	} else if !os.IsNotExist(readErr) {
		return NamedResult{}, readErr
	} else {
		envelope, key, e := sealed.Encrypt([]byte(text))
		if e != nil {
			return NamedResult{}, e
		}
		sha := sha256.Sum256(envelope)
		saved = outbox{Server: c.Server, Account: keys.state.Account, Root: keys.state.Root, SHA256: hex.EncodeToString(sha[:]), Key: key, Envelope: envelope, Secrets: map[string]string{}}
		for _, email := range recipients {
			id := enc.EncodeToString(random(24))
			secret := random(32)
			saved.Invitations = append(saved.Invitations, Invitation{ID: id, Email: email, Commitment: digest(secret)})
			saved.Secrets[id] = enc.EncodeToString(secret)
		}
	}
	if err = privateWrite(pending, saved); err != nil {
		return NamedResult{}, err
	}
	key, err := decode(saved.Key, 32)
	if err != nil {
		return NamedResult{}, err
	}
	ownerWrap, err := wrap(keys.state.Inbox, key, namedContext(saved.SHA256, saved.Account, saved.Root, keys.state.Epoch))
	if err != nil {
		return NamedResult{}, err
	}
	p := policy{Account: saved.Account, Root: saved.Root, Head: keys.remote.Record.head(), Signer: keys.own.ID, SHA256: saved.SHA256, Inbox: keys.state.Inbox, Epoch: keys.state.Epoch, Wrap: ownerWrap, Recipients: saved.Invitations}
	signed, err := sign("named-policy", p, keys.device.key())
	if err != nil {
		return NamedResult{}, err
	}
	header, _ := json.Marshal(signed)
	var ack struct {
		ID      string `json:"id"`
		SHA256  string `json:"sha256"`
		Account string `json:"account_id"`
	}
	if err = c.namedRequest(ctx, "POST", "/api/named-shares", map[string]string{"content-type": "application/vnd.session-link.encrypted", "x-slink-policy": string(header)}, bytes.NewReader(saved.Envelope), &ack); err != nil {
		return NamedResult{}, err
	}
	if !namedID.MatchString(ack.ID) || ack.SHA256 != saved.SHA256 || ack.Account != saved.Account {
		return NamedResult{}, errors.New("server did not acknowledge this private share; retry with the saved local keys")
	}
	saved.ID = ack.ID
	saved.Envelope = nil
	if err = privateWrite(c.outboxFile(saved.SHA256), saved); err != nil {
		return NamedResult{}, err
	}
	_ = os.Remove(pending)
	result := saved.result()
	if _, err := c.handle(ctx, "sync", Input{}); err != nil {
		result.Error = "Published; private invitation backup needs a retry."
	}
	return result, nil
}
func (c Client) finalizeNamed(ctx context.Context, saved outbox, keys nativeKeys) (NamedResult, error) {
	out := saved.result()
	var remote namedRemote
	if err := c.namedRequest(ctx, "GET", "/api/named-shares/"+saved.ID, nil, nil, &remote); err != nil {
		return out, err
	}
	if !remote.Owner || remote.ID != saved.ID || remote.SHA256 != saved.SHA256 || saved.Root != keys.state.Root {
		return out, errors.New("private share identity changed")
	}
	for _, recipient := range remote.Recipients {
		var local *NamedRecipient
		for i := range out.Recipients {
			if out.Recipients[i].ID == recipient.ID && out.Recipients[i].Email == recipient.Email {
				local = &out.Recipients[i]
				break
			}
		}
		if local == nil {
			return out, errors.New("recipient list changed")
		}
		local.Status = recipient.Status
		if saved.Status == nil {
			saved.Status = map[string]string{}
		}
		saved.Status[recipient.ID] = recipient.Status
		if recipient.Status != "accepted" || recipient.Claim == nil {
			continue
		}
		signed := recipient.Claim
		var binding claim
		if signed.read(&binding) != nil {
			return out, errors.New("invalid recipient identity binding")
		}
		raw, _ := decode(signed.Payload, 0)
		secret, err := decode(saved.Secrets[recipient.ID], 32)
		if err != nil {
			return out, err
		}
		mac := hmac.New(sha256.New, secret)
		mac.Write([]byte("slink/recipient-binding/v1\x00"))
		mac.Write(raw)
		submitted, err := decode(signed.Proof, 32)
		if err != nil || !hmac.Equal(mac.Sum(nil), submitted) || binding.InviteID != recipient.ID || binding.ShareID != saved.ID || binding.SHA256 != saved.SHA256 || binding.Owner != saved.Account || binding.OwnerRoot != saved.Root {
			return out, errors.New("recipient verification failed; no decryption key was released")
		}
		// Fetch one recipient history at a time; a ten-person share must
		// not multiply the bounded identity response by ten.
		if len(recipient.History) == 0 {
			var proof namedRemote
			if err := c.namedRequest(ctx, "GET", "/api/named-shares/"+saved.ID+"?invite="+recipient.ID, nil, nil, &proof); err != nil {
				return out, err
			}
			for _, item := range proof.Recipients {
				if item.ID == recipient.ID {
					recipient.History = item.History
					break
				}
			}
		}
		state, err := verifyHistory(recipient.History, binding.Account)
		if err != nil {
			return out, err
		}
		if state.Root != binding.Root || state.Inbox == "" {
			return out, errors.New("recipient's encryption identity changed")
		}
		index := -1
		for i, event := range recipient.History {
			if event.hash() == binding.Head {
				index = i
				break
			}
		}
		if index < 0 {
			return out, errors.New("recipient history conflicts with its invitation")
		}
		then, err := verifyHistory(recipient.History[:index+1], binding.Account)
		if err != nil {
			return out, err
		}
		device, ok := then.device(binding.Signer)
		if !ok || then.Inbox != binding.Inbox || then.Epoch != binding.Epoch || signed.verify("recipient-claim", device.Sign) != nil {
			return out, errors.New("recipient request is not from an approved device")
		}
		key, err := decode(saved.Key, 32)
		if err != nil {
			return out, err
		}
		wrapped, err := wrap(state.Inbox, key, namedContext(saved.SHA256, state.Account, state.Root, state.Epoch))
		if err != nil {
			return out, err
		}
		grant := namedGrant{Account: saved.Account, Root: saved.Root, Head: keys.remote.Record.head(), Signer: keys.own.ID, ShareID: saved.ID, InviteID: recipient.ID, SHA256: saved.SHA256, Recipient: state.Account, RecipientRoot: state.Root, Inbox: state.Inbox, Epoch: state.Epoch, Wrap: wrapped, ClaimHash: signed.hash()}
		authorization, err := sign("named-grant", grant, keys.device.key())
		if err != nil {
			return out, err
		}
		if err = c.namedPost(ctx, saved.ID, map[string]any{"action": "grant", "grant": authorization}, nil); err != nil {
			return out, err
		}
		local.Status = "ready"
		saved.Status[recipient.ID] = "ready"
	}
	if err := privateWrite(c.outboxFile(saved.SHA256), saved); err != nil {
		return out, err
	}
	return out, nil
}
func (c Client) NamedShares(ctx context.Context, revokeID, invitationID string, refresh bool) ([]NamedResult, error) {
	operations.Lock()
	defer operations.Unlock()
	c.Server = strings.TrimRight(c.Server, "/")
	release, err := c.lockNative()
	if err != nil {
		return nil, err
	}
	defer release()
	// No network when this machine has never created named shares.
	entries, err := os.ReadDir(filepath.Join(c.Home, "named-shares"))
	if os.IsNotExist(err) {
		return []NamedResult{}, nil
	}
	if err != nil {
		return nil, err
	}
	keys, err := c.nativeKeys(ctx, false)
	if err != nil {
		return nil, err
	}
	if revokeID != "" {
		if !namedID.MatchString(revokeID) {
			return nil, errors.New("invalid share id")
		}
		if err = c.namedPost(ctx, revokeID, map[string]any{"action": "revoke", "invite_id": invitationID}, nil); err != nil {
			return nil, err
		}
	}
	results := []NamedResult{}
	// Rotate a bounded background batch so unanswered invitations cannot
	// starve later recipients or exhaust the hosted read limit.
	if !refresh && len(entries) > 0 {
		start := int(time.Now().Unix()/15*8) % len(entries)
		entries = append(entries[start:], entries[:start]...)
	}
	processed := 0
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") || strings.HasPrefix(entry.Name(), "pending-") {
			continue
		}
		raw, err := os.ReadFile(filepath.Join(c.Home, "named-shares", entry.Name()))
		if err != nil {
			return nil, err
		}
		var saved outbox
		if json.Unmarshal(raw, &saved) != nil {
			return nil, errors.New("cannot read saved private share")
		}
		if saved.Server != c.Server || saved.Account != keys.state.Account {
			continue
		}
		if !validOutbox(saved, c.Server, keys.state.Account, keys.state.Root) {
			return nil, errors.New("invalid saved private share")
		}
		terminal := len(saved.Status) == len(saved.Invitations)
		for _, value := range saved.Status {
			if value != "ready" && value != "revoked" && value != "expired" {
				terminal = false
			}
		}
		if terminal && !refresh {
			continue
		}
		if !refresh && processed >= 8 {
			break
		}
		processed++
		result, err := c.finalizeNamed(ctx, saved, keys)
		if err != nil {
			result.Error = err.Error()
		}
		results = append(results, result)
	}
	return results, nil
}

func validOutbox(item outbox, server, account, root string) bool {
	if item.Server != server || item.Account != account || item.Root != root || !namedID.MatchString(item.ID) || len(item.SHA256) != 64 || len(item.Envelope) > 0 || len(item.Invitations) < 1 || len(item.Invitations) > 10 {
		return false
	}
	if _, err := hex.DecodeString(item.SHA256); err != nil {
		return false
	}
	if _, err := decode(item.Key, 32); err != nil {
		return false
	}
	seen := map[string]bool{}
	for _, r := range item.Invitations {
		if seen[r.ID] || len(r.Email) > 254 || !emailAddress.MatchString(r.Email) {
			return false
		}
		seen[r.ID] = true
		if _, err := decode(r.ID, 24); err != nil {
			return false
		}
		secret, err := decode(item.Secrets[r.ID], 32)
		if err != nil || digest(secret) != r.Commitment {
			return false
		}
	}
	return true
}
