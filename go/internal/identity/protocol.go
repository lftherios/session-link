package identity

import (
	"encoding/base32"
	"errors"
	"fmt"
	"strings"
)

type Device struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	Sign string `json:"sign"`
	Box  string `json:"box"`
	Wrap string `json:"wrap"`
}
type State struct {
	Account      string   `json:"account"`
	Seq          int      `json:"seq"`
	Prev         string   `json:"prev"`
	Kind         string   `json:"kind"`
	Signer       string   `json:"signer"`
	Root         string   `json:"root"`
	Recovery     string   `json:"recovery"`
	RecoveryBox  string   `json:"recovery_box"`
	RecoveryWrap string   `json:"recovery_wrap"`
	Epoch        int      `json:"epoch"`
	Devices      []Device `json:"devices"`
}
type Vault struct {
	Account  string `json:"account"`
	Revision int    `json:"revision"`
	Head     string `json:"head"`
	Epoch    int    `json:"epoch"`
	Signer   string `json:"signer"`
	Data     string `json:"data"`
}
type Record struct {
	Events []Signed `json:"events"`
	Vault  Signed   `json:"vault"`
}
type Request struct {
	Account string `json:"account"`
	Root    string `json:"root"`
	Name    string `json:"name"`
	Sign    string `json:"sign"`
	Box     string `json:"box"`
	Nonce   string `json:"nonce"`
	Created int64  `json:"created"`
}

func deviceID(sign, box string) string { return digest([]byte(sign + "." + box)) }
func (r Request) id() string           { return deviceID(r.Sign, r.Box) }
func (s State) device(id string) (Device, bool) {
	for _, d := range s.Devices {
		if d.ID == id {
			return d, true
		}
	}
	return Device{}, false
}
func (s State) context() string { return fmt.Sprintf("%s/%s/%d", s.Account, s.Root, s.Epoch) }
func (r Record) head() string {
	if len(r.Events) == 0 {
		return ""
	}
	return r.Events[len(r.Events)-1].hash()
}
func (r Record) verify(account string) (State, Vault, error) {
	var previous State
	var vault Vault
	var head string
	bad := func() (State, Vault, error) { return State{}, Vault{}, errors.New("invalid signed device history") }
	if len(r.Events) == 0 || len(r.Events) > 512 {
		return bad()
	}
	for i, event := range r.Events {
		var next State
		if event.read(&next) != nil || next.Account != account || next.Seq != i || next.Prev != head || next.Epoch < 1 || len(next.Devices) < 1 || len(next.Devices) > 32 {
			return bad()
		}
		if _, err := decode(next.Root, 32); err != nil {
			return bad()
		}
		if _, err := decode(next.Recovery, 32); err != nil {
			return bad()
		}
		seen := map[string]bool{}
		for _, d := range next.Devices {
			if d.ID != deviceID(d.Sign, d.Box) || seen[d.ID] || len(d.Name) > 80 || len(d.Wrap) != 124 {
				return bad()
			}
			if _, err := decode(d.Sign, 32); err != nil {
				return bad()
			}
			if _, err := decode(d.Box, 32); err != nil {
				return bad()
			}
			seen[d.ID] = true
		}
		if len(next.RecoveryWrap) != 124 || len(next.RecoveryBox) != 123 {
			return bad()
		}
		if i == 0 {
			if next.Kind != "init" || next.Signer != "root" || next.Epoch != 1 || len(next.Devices) != 1 || event.verify("event", next.Root) != nil {
				return bad()
			}
		} else {
			if next.Root != previous.Root || next.Recovery != previous.Recovery || next.RecoveryBox != previous.RecoveryBox {
				return bad()
			}
			pub := previous.Root
			if next.Kind != "recover" {
				d, ok := previous.device(next.Signer)
				if !ok {
					return bad()
				}
				pub = d.Sign
			} else if next.Signer != "root" {
				return bad()
			}
			if event.verify("event", pub) != nil {
				return bad()
			}
			added, removed := 0, 0
			for _, d := range next.Devices {
				old, ok := previous.device(d.ID)
				if !ok {
					added++
					continue
				}
				if d.Name != old.Name || d.Sign != old.Sign || d.Box != old.Box {
					return bad()
				}
				if next.Kind != "revoke" && d.Wrap != old.Wrap {
					return bad()
				}
			}
			for _, d := range previous.Devices {
				if _, ok := next.device(d.ID); !ok {
					removed++
				}
			}
			switch next.Kind {
			case "approve", "recover":
				if added != 1 || removed != 0 || next.Epoch != previous.Epoch || next.RecoveryWrap != previous.RecoveryWrap {
					return bad()
				}
			case "revoke":
				if added != 0 || removed != 1 || next.Epoch != previous.Epoch+1 {
					return bad()
				}
				if next.RecoveryWrap == previous.RecoveryWrap {
					return bad()
				}
				for _, d := range next.Devices {
					old, _ := previous.device(d.ID)
					if d.Wrap == old.Wrap {
						return bad()
					}
				}
				if _, ok := next.device(next.Signer); !ok {
					return bad()
				}
			default:
				return bad()
			}
		}
		previous, head = next, event.hash()
	}
	if r.Vault.read(&vault) != nil || vault.Account != account || vault.Head != head || vault.Epoch != previous.Epoch || vault.Revision < 1 || len(vault.Data) > 6*1024*1024 {
		return bad()
	}
	pub := previous.Root
	if vault.Signer != "root" {
		d, ok := previous.device(vault.Signer)
		if !ok {
			return bad()
		}
		pub = d.Sign
	} else if previous.Kind != "init" && previous.Kind != "recover" {
		return bad()
	}
	if err := r.Vault.verify("vault", pub); err != nil {
		return State{}, Vault{}, err
	}
	return previous, vault, nil
}
func confirmation(s Signed) string {
	raw, _ := decode(s.hash(), 32)
	code := base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(raw)[:12]
	return code[:4] + "-" + code[4:8] + "-" + code[8:]
}
func normalizeCode(s string) string {
	return strings.ToUpper(strings.NewReplacer("-", "", " ", "").Replace(s))
}
