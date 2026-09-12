package identity

import (
	"bytes"
	"context"
	"crypto/ecdh"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode/utf8"
)

// All native callers share this lock. Device mutations use server CAS as well.
var operations sync.Mutex

type Client struct{ Home, Server, APIKey string }
type localDevice struct {
	Seed            []byte  `json:"seed"`
	Box             []byte  `json:"box"`
	Root            string  `json:"root"`
	Seq             int     `json:"seq"`
	Head            string  `json:"head"`
	Revision        int     `json:"revision"`
	Request         *Signed `json:"request,omitempty"`
	PendingRecovery string  `json:"pending_recovery,omitempty"`
}
type OwnedShare struct {
	ID     string `json:"id"`
	SHA256 string `json:"sha256"`
}
type remote struct {
	Owned    []OwnedShare `json:"owned"`
	Account  string       `json:"account"`
	Record   *Record      `json:"record"`
	Requests []Signed     `json:"requests"`
}
type Receipt struct {
	Server  string `json:"server"`
	Account string `json:"account"`
	Key     string `json:"key"`
	SHA256  string `json:"sha256"`
	URL     string `json:"url"`
}
type contents struct {
	Version int       `json:"version"`
	Shares  []Receipt `json:"shares"`
}
type Input struct {
	Name        string `json:"name"`
	ID          string `json:"id"`
	Code        string `json:"code"`
	RecoveryKey string `json:"recovery_key"`
}
type Pending struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	Code string `json:"code"`
}
type Status struct {
	Server          string       `json:"server"`
	Account         string       `json:"account"`
	State           string       `json:"state"`
	DeviceID        string       `json:"device_id,omitempty"`
	Devices         []DeviceInfo `json:"devices"`
	Requests        []Pending    `json:"requests"`
	Shares          []Receipt    `json:"shares"`
	Confirmation    string       `json:"confirmation,omitempty"`
	RecoveryKey     string       `json:"recovery_key,omitempty"`
	RecoveryPending bool         `json:"recovery_pending"`
}
type DeviceInfo struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Current bool   `json:"current"`
}
type httpError struct {
	status  int
	message string
}

func (e *httpError) Error() string { return e.message }
func (c Client) request(ctx context.Context, body any) (remote, error) {
	var out remote
	u, err := url.Parse(c.Server)
	if err != nil || u.Host == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" || (u.Scheme != "https" && !(u.Scheme == "http" && (u.Hostname() == "localhost" || net.ParseIP(u.Hostname()).IsLoopback()))) {
		return out, errors.New("invalid encrypted identity server")
	}
	method := "GET"
	var reader io.Reader
	if body != nil {
		raw, err := json.Marshal(body)
		if err != nil {
			return out, err
		}
		reader = bytes.NewReader(raw)
		method = "POST"
	}
	req, err := http.NewRequestWithContext(ctx, method, strings.TrimRight(c.Server, "/")+"/api/identity", reader)
	if err != nil {
		return out, err
	}
	req.Header.Set("authorization", "Bearer "+c.APIKey)
	req.Header.Set("content-type", "application/json")
	res, err := (&http.Client{Timeout: 45 * time.Second, CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse }}).Do(req)
	if err != nil {
		return out, errors.New("cannot reach encrypted key backup")
	}
	defer res.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(res.Body, 12*1024*1024+1))
	if err != nil || len(raw) > 12*1024*1024 {
		return out, errors.New("invalid identity response")
	}
	if res.StatusCode != 200 {
		var failure struct {
			Error struct {
				Message string `json:"message"`
			} `json:"error"`
		}
		json.Unmarshal(raw, &failure)
		return out, &httpError{res.StatusCode, fmt.Sprintf("%s (HTTP %d)", failure.Error.Message, res.StatusCode)}
	}
	if json.Unmarshal(raw, &out) != nil || out.Account == "" || len(out.Account) > 128 {
		return out, errors.New("invalid identity response")
	}
	return out, nil
}
func privateWrite(file string, value any) error {
	raw, err := json.Marshal(value)
	if err != nil {
		return err
	}
	dir := filepath.Dir(file)
	if err = os.MkdirAll(dir, 0700); err != nil {
		return err
	}
	f, err := os.CreateTemp(dir, ".identity-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err = f.Write(raw); err == nil {
		err = f.Sync()
	}
	closed := f.Close()
	if err != nil {
		return err
	}
	if closed != nil {
		return closed
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
func (c Client) file(account string) string {
	return filepath.Join(c.Home, "identity", digest([]byte(c.Server+"\x00"+account))+".json")
}
func (c Client) load(account string) (*localDevice, error) {
	raw, err := os.ReadFile(c.file(account))
	if os.IsNotExist(err) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var d localDevice
	if json.Unmarshal(raw, &d) != nil || len(d.Seed) != 32 || len(d.Box) != 32 {
		return nil, errors.New("cannot read local device keys")
	}
	return &d, nil
}
func newDevice() (*localDevice, error) {
	box, err := ecdh.X25519().GenerateKey(rand.Reader)
	if err != nil {
		return nil, err
	}
	return &localDevice{Seed: random(32), Box: box.Bytes(), Seq: -1}, nil
}
func (d *localDevice) key() ed25519.PrivateKey { return ed25519.NewKeyFromSeed(d.Seed) }
func (d *localDevice) public(name string) Device {
	key, _ := ecdh.X25519().NewPrivateKey(d.Box)
	v := Device{Name: name, Sign: enc.EncodeToString(d.key().Public().(ed25519.PublicKey)), Box: enc.EncodeToString(key.PublicKey().Bytes())}
	v.ID = deviceID(v.Sign, v.Box)
	return v
}
func (c Client) pin(d *localDevice, r Record, state State, v Vault) error {
	if d.Root != "" && d.Root != state.Root {
		return errors.New("server recovery identity changed; refusing replacement")
	}
	if d.Seq >= len(r.Events) || d.Revision > v.Revision {
		return errors.New("server returned an older encrypted vault")
	}
	if d.Seq >= 0 && d.Head != "" && r.Events[d.Seq].hash() != d.Head {
		return errors.New("device history conflicts with this device's saved history")
	}
	d.Root, d.Seq, d.Head, d.Revision = state.Root, state.Seq, r.head(), v.Revision
	return c.save(d, state.Account)
}
func (c Client) save(d *localDevice, account string) error { return privateWrite(c.file(account), d) }
func validReceipt(r Receipt, server, account string) bool {
	if r.Server != server || r.Account != account || len(r.SHA256) != 64 {
		return false
	}
	for _, ch := range r.SHA256 {
		if !strings.ContainsRune("0123456789abcdef", ch) {
			return false
		}
	}
	if _, err := decode(r.Key, 32); err != nil {
		return false
	}
	u, err := url.Parse(r.URL)
	base, e := url.Parse(server)
	if err != nil || e != nil || u.Scheme != base.Scheme || u.Host != base.Host || u.User != nil || u.RawQuery != "" || !strings.HasPrefix(u.Path, base.Path+"/s/") {
		return false
	}
	id := strings.TrimPrefix(u.Path, base.Path+"/s/")
	if len(id) != 14 {
		return false
	}
	for _, ch := range id {
		if !strings.ContainsRune("23456789abcdefghjkmnpqrstuvwxyz", ch) {
			return false
		}
	}
	return u.Fragment == "key="+r.Key
}
func (c Client) decrypt(key []byte, state State, v Vault) (contents, error) {
	raw, err := unseal(key, v.Data, "slink/vault/v1/"+state.context())
	if err != nil {
		return contents{}, errors.New("cannot decrypt encrypted key vault")
	}
	var data contents
	if len(raw) > 4*1024*1024 || json.Unmarshal(raw, &data) != nil || data.Version != 1 || len(data.Shares) > 10000 {
		return contents{}, errors.New("invalid key vault")
	}
	for _, receipt := range data.Shares {
		if !validReceipt(receipt, c.Server, state.Account) {
			return contents{}, errors.New("invalid share key in vault")
		}
	}
	return data, nil
}
func (c Client) merge(data contents, account string, owned []OwnedShare) (contents, error) {
	entries, err := os.ReadDir(filepath.Join(c.Home, "shares"))
	if err != nil && !os.IsNotExist(err) {
		return data, err
	}
	byURL := map[string]Receipt{}
	for _, r := range data.Shares {
		byURL[r.URL] = r
	}
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") {
			continue
		}
		raw, err := os.ReadFile(filepath.Join(c.Home, "shares", entry.Name()))
		if err != nil {
			return data, err
		}
		var receipt Receipt
		// Upgrade old receipts only when this authenticated account owns the
		// exact ciphertext. Operator-key and other-account receipts stay local.
		if json.Unmarshal(raw, &receipt) != nil {
			continue
		}
		if receipt.Account == "" && receipt.Server == c.Server {
			for _, share := range owned {
				if receipt.SHA256 == share.SHA256 && receipt.URL == c.Server+"/s/"+share.ID+"#key="+receipt.Key {
					receipt.Account = account
					break
				}
			}
		}
		if validReceipt(receipt, c.Server, account) {
			byURL[receipt.URL] = receipt
		}
	}
	data.Version = 1
	data.Shares = []Receipt{}
	for _, receipt := range byURL {
		data.Shares = append(data.Shares, receipt)
	}
	sort.Slice(data.Shares, func(i, j int) bool { return data.Shares[i].URL < data.Shares[j].URL })
	if len(data.Shares) > 10000 {
		return data, errors.New("key vault is full")
	}
	return data, nil
}
func (c Client) importShares(data contents) error {
	for _, r := range data.Shares {
		file := filepath.Join(c.Home, "shares", digest([]byte(r.Server+"\x00"+r.Account+"\x00"+r.URL))+".json")
		raw, _ := json.Marshal(r)
		if existing, err := os.ReadFile(file); err == nil && bytes.Equal(raw, existing) {
			if info, err := os.Stat(file); err == nil && info.Mode().Perm() == 0600 {
				continue
			}
		}

		if err := privateWrite(file, r); err != nil {
			return err
		}
	}
	return nil
}
func (c Client) commit(ctx context.Context, old *Record, next State, event *Signed, data contents, key []byte, signer ed25519.PrivateKey) (remote, error) {
	raw, err := json.Marshal(data)
	if err != nil {
		return remote{}, err
	}
	if len(raw) > 4*1024*1024 {
		return remote{}, errors.New("key vault exceeds the backup limit")
	}
	encrypted, err := seal(key, raw, "slink/vault/v1/"+next.context())
	if err != nil {
		return remote{}, err
	}
	rev := 0
	head := ""
	if old != nil {
		var previous Vault
		old.Vault.read(&previous)
		rev = previous.Revision
		head = old.head()
	}
	nextHead := head
	if event != nil {
		nextHead = event.hash()
	}
	vault, err := sign("vault", Vault{Account: next.Account, Revision: rev + 1, Head: nextHead, Epoch: next.Epoch, Signer: next.Signer, Data: encrypted}, signer)
	if err != nil {
		return remote{}, err
	}
	body := map[string]any{"expected_head": head, "expected_revision": rev, "vault": vault}
	if event != nil {
		body["event"] = event
	}
	return c.request(ctx, body)
}
func (c Client) Handle(ctx context.Context, action string, in Input) (Status, error) {
	operations.Lock()
	defer operations.Unlock()
	if err := os.MkdirAll(filepath.Join(c.Home, "identity"), 0700); err != nil {
		return Status{}, err
	}
	lock, err := os.OpenFile(filepath.Join(c.Home, "identity", ".lock"), os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return Status{}, err
	}
	defer lock.Close()
	if err = lockDeviceFile(lock); err != nil {
		return Status{}, errors.New("another slink process is updating device keys; retry shortly")
	}

	c.Server = strings.TrimRight(c.Server, "/")
	// A vault conflict is retried by fetching and merging the latest receipts.
	for attempts := 0; attempts < 3; attempts++ {
		out, err := c.handle(ctx, action, in)
		var conflict *httpError
		if errors.As(err, &conflict) && conflict.status == 409 && action == "sync" {
			continue
		}
		return out, err
	}
	return Status{}, errors.New("vault is changing on another device; retry backup")
}
func (c Client) handle(ctx context.Context, action string, in Input) (Status, error) {
	remote, err := c.request(ctx, nil)
	if err != nil {
		return Status{}, err
	}
	d, err := c.load(remote.Account)
	if err != nil {
		return Status{}, err
	}
	result := Status{Server: c.Server, Account: remote.Account, State: "setup", Devices: []DeviceInfo{}, Requests: []Pending{}, Shares: []Receipt{}}
	if remote.Record == nil {
		if d != nil && d.Root != "" {
			return result, errors.New("encrypted identity is missing from the server; refusing to replace it")
		}
		if action == "status" {
			return result, nil
		}
		if action != "setup" {
			return result, errors.New("set up recovery on your first device")
		}
		if d == nil {
			d, err = newDevice()
			if err != nil {
				return result, err
			}
		}
		seed := random(32)
		root := ed25519.NewKeyFromSeed(seed)
		recovery, err := ecdh.X25519().GenerateKey(rand.Reader)
		if err != nil {
			return result, err
		}
		secret := random(32)
		key := random(32)
		state := State{Account: remote.Account, Kind: "init", Signer: "root", Root: enc.EncodeToString(root.Public().(ed25519.PublicKey)), Recovery: enc.EncodeToString(recovery.PublicKey().Bytes()), Epoch: 1}
		state.RecoveryBox, err = seal(secret, append(seed, recovery.Bytes()...), "slink/recovery/v1/"+state.Account+"/"+state.Root)
		if err != nil {
			return result, err
		}
		first := d.public(deviceName(in.Name))
		first.Wrap, err = wrap(first.Box, key, state.context())
		if err != nil {
			return result, err
		}
		state.Devices = []Device{first}
		state.RecoveryWrap, err = wrap(state.Recovery, key, state.context())
		if err != nil {
			return result, err
		}
		event, err := sign("event", state, root)
		if err != nil {
			return result, err
		}
		data, err := c.merge(contents{}, state.Account, remote.Owned)
		if err != nil {
			return result, err
		}
		// Save before contacting the server. A lost response must not lose recovery.
		d.PendingRecovery = "slr1." + digest([]byte(state.Account+"\x00"+state.Root)) + "." + enc.EncodeToString(secret)
		if err = c.save(d, state.Account); err != nil {
			return result, err
		}
		remote, err = c.commit(ctx, nil, state, &event, data, key, root)
		if err != nil {
			return result, err
		}
	}
	state, vault, err := remote.Record.verify(remote.Account)
	if err != nil {
		return result, err
	}
	if d != nil {
		if err = c.pin(d, *remote.Record, state, vault); err != nil {
			return result, err
		}
	}
	if d == nil && (action == "request" || action == "recover") {
		d, err = newDevice()
		if err != nil {
			return result, err
		}
		if err = c.pin(d, *remote.Record, state, vault); err != nil {
			return result, err
		}
	}
	own := Device{}
	approved := false
	if d != nil {
		own, approved = state.device(d.public("").ID)
		result.DeviceID = d.public("").ID
		result.RecoveryPending = d.PendingRecovery != ""
	}
	result.State = "locked"
	for _, device := range state.Devices {
		result.Devices = append(result.Devices, DeviceInfo{device.ID, device.Name, device.ID == result.DeviceID})
	}
	for _, signed := range remote.Requests {
		var request Request
		if signed.read(&request) != nil || request.Account != state.Account || request.Root != state.Root || signed.verify("request", request.Sign) != nil || request.Created < time.Now().Add(-10*time.Minute).UnixMilli() {
			continue
		}
		result.Requests = append(result.Requests, Pending{request.id(), request.Name, confirmation(signed)})
		if request.id() == result.DeviceID {
			result.Confirmation = confirmation(signed)
		}
	}
	if action == "request" && !approved {
		request := Request{Account: state.Account, Root: state.Root, Name: deviceName(in.Name), Sign: d.public("").Sign, Box: d.public("").Box, Nonce: enc.EncodeToString(random(24)), Created: time.Now().UnixMilli()}
		signed, err := sign("request", request, d.key())
		if err != nil {
			return result, err
		}
		d.Request = &signed
		if err = c.save(d, state.Account); err != nil {
			return result, err
		}
		_, err = c.request(ctx, map[string]any{"request": signed})
		result.Confirmation = confirmation(signed)
		return result, err
	}
	if action == "recover" && !approved {
		parts := strings.Split(strings.TrimSpace(in.RecoveryKey), ".")
		if len(parts) != 3 || parts[0] != "slr1" || parts[1] != digest([]byte(state.Account+"\x00"+state.Root)) {
			return result, errors.New("recovery key does not match this account's recovery identity")
		}
		secret, err := decode(parts[2], 32)
		if err != nil {
			return result, err
		}
		private, err := unseal(secret, state.RecoveryBox, "slink/recovery/v1/"+state.Account+"/"+state.Root)
		if err != nil || len(private) != 64 {
			return result, errors.New("incorrect recovery key")
		}
		root := ed25519.NewKeyFromSeed(private[:32])
		recovery, err := ecdh.X25519().NewPrivateKey(private[32:])
		if err != nil || enc.EncodeToString(root.Public().(ed25519.PublicKey)) != state.Root || enc.EncodeToString(recovery.PublicKey().Bytes()) != state.Recovery {
			return result, errors.New("recovery identity verification failed")
		}
		key, err := unwrap(private[32:], state.RecoveryWrap, state.context())
		if err != nil {
			return result, err
		}
		data, err := c.decrypt(key, state, vault)
		if err != nil {
			return result, err
		}
		data, err = c.merge(data, state.Account, remote.Owned)
		if err != nil {
			return result, err
		}
		// Fresh keys on recovery also cover recovery from a previously revoked device.
		d, err = newDevice()
		if err != nil {
			return result, err
		}
		if err = c.pin(d, *remote.Record, state, vault); err != nil {
			return result, err
		}
		added := d.public(deviceName(in.Name))
		added.Wrap, err = wrap(added.Box, key, state.context())
		if err != nil {
			return result, err
		}
		state.Devices = append(state.Devices, added)
		state.Kind, state.Signer, state.Seq, state.Prev = "recover", "root", state.Seq+1, remote.Record.head()
		event, err := sign("event", state, root)
		if err != nil {
			return result, err
		}
		if _, err = c.commit(ctx, remote.Record, state, &event, data, key, root); err != nil {
			return result, err
		}
		return c.handle(ctx, "status", Input{})
	}
	if !approved {
		if action != "status" {
			return result, ErrLocked
		}
		return result, nil
	}
	result.State = "ready"
	key, err := unwrap(d.Box, own.Wrap, state.context())
	if err != nil {
		return result, err
	}
	data, err := c.decrypt(key, state, vault)
	if err != nil {
		return result, err
	}
	if err = c.importShares(data); err != nil {
		return result, err
	}
	result.Shares = data.Shares
	if action == "setup" && d.PendingRecovery != "" {
		result.RecoveryKey = d.PendingRecovery
		return result, nil
	}
	if action == "confirm-recovery" {
		d.PendingRecovery = ""
		if err = c.save(d, state.Account); err != nil {
			return result, err
		}
		result.RecoveryPending = false
		return result, nil
	}
	if action == "status" {
		return result, nil
	}
	if action != "sync" && action != "approve" && action != "revoke" {
		return result, errors.New("unknown device action")
	}
	data, err = c.merge(data, state.Account, remote.Owned)
	if err != nil {
		return result, err
	}
	signer := d.key()
	state.Signer = own.ID
	var event *Signed
	if action == "approve" {
		var pending *Signed
		var request Request
		for _, signed := range remote.Requests {
			var p Request
			if signed.read(&p) == nil && p.id() == in.ID {
				copy := signed
				pending = &copy
				request = p
				break
			}
		}
		if pending == nil || request.Account != state.Account || request.Root != state.Root || pending.verify("request", request.Sign) != nil || normalizeCode(in.Code) != normalizeCode(confirmation(*pending)) {
			return result, errors.New("device code does not match; check the code on your new device")
		}
		added := Device{ID: request.id(), Name: request.Name, Sign: request.Sign, Box: request.Box}
		added.Wrap, err = wrap(added.Box, key, state.context())
		if err != nil {
			return result, err
		}
		state.Devices = append(state.Devices, added)
	} else if action == "revoke" {
		if in.ID == own.ID {
			return result, errors.New("revoke this device from another approved device")
		}
		if _, ok := state.device(in.ID); !ok {
			return result, errors.New("device is no longer approved")
		}
		state.Epoch++
		key = random(32)
		devices := []Device{}
		for _, device := range state.Devices {
			if device.ID == in.ID {
				continue
			}
			device.Wrap, err = wrap(device.Box, key, state.context())
			if err != nil {
				return result, err
			}
			devices = append(devices, device)
		}
		state.Devices = devices
		state.RecoveryWrap, err = wrap(state.Recovery, key, state.context())
		if err != nil {
			return result, err
		}
	}
	if action != "sync" {
		state.Kind = action
		state.Seq++
		state.Prev = remote.Record.head()
		signed, err := sign("event", state, signer)
		if err != nil {
			return result, err
		}
		event = &signed
	}
	if _, err = c.commit(ctx, remote.Record, state, event, data, key, signer); err != nil {
		return result, err
	}
	return c.handle(ctx, "status", Input{})
}
func deviceName(name string) string {
	name = strings.TrimSpace(name)
	if name == "" {
		return "My device"
	}
	for len(name) > 80 {
		_, n := utf8.DecodeLastRuneInString(name)
		name = name[:len(name)-n]
	}
	return name
}
