package identity

import (
	"crypto/ed25519"
	"crypto/rand"
	"testing"
)

func TestRetiredIncomingKeyCannotBeReinstated(t *testing.T) {
	rootPublic, rootKey, _ := ed25519.GenerateKey(rand.Reader)
	laptopPublic, laptopKey, _ := ed25519.GenerateKey(rand.Reader)
	encoded := func(n int) string { return enc.EncodeToString(random(n)) }
	wrapper := func() string { return encoded(32) + "." + encoded(60) }
	device := func(name, sign string) Device {
		d := Device{Name: name, Sign: sign, Box: encoded(32), Wrap: wrapper()}
		d.ID = deviceID(d.Sign, d.Box)
		return d
	}
	extra := func() Device {
		public, _, _ := ed25519.GenerateKey(rand.Reader)
		return device("Extra", enc.EncodeToString(public))
	}
	laptop := device("Laptop", enc.EncodeToString(laptopPublic))
	state := State{Account: "usr_fixture", Kind: "init", Signer: "root", Root: enc.EncodeToString(rootPublic), Recovery: encoded(32), RecoveryBox: encoded(92), RecoveryWrap: wrapper(), Epoch: 1, Devices: []Device{laptop}}
	initial, err := sign("event", state, rootKey)
	if err != nil {
		t.Fatal(err)
	}
	events := []Signed{initial}
	// step appends one laptop-signed event derived from the current head.
	step := func(events []Signed, state State, change func(*State)) ([]Signed, State) {
		next := state
		next.Devices = append([]Device(nil), state.Devices...)
		change(&next)
		next.Seq, next.Prev, next.Signer = state.Seq+1, events[len(events)-1].hash(), laptop.ID
		event, err := sign("event", next, laptopKey)
		if err != nil {
			t.Fatal(err)
		}
		return append(append([]Signed(nil), events...), event), next
	}
	approve := func(s *State) {
		s.Kind = "approve"
		s.Devices = append(s.Devices, extra())
	}
	rotate := func(inbox string) func(*State) {
		return func(s *State) {
			s.Kind, s.Epoch, s.Inbox, s.RecoveryWrap = "revoke", s.Epoch+1, inbox, wrapper()
			s.Devices = []Device{s.Devices[0]}
			s.Devices[0].Wrap = wrapper()
		}
	}
	a, b := encoded(32), encoded(32)
	events, state = step(events, state, func(s *State) { s.Kind, s.Inbox = "sharing", a })
	events, state = step(events, state, approve)
	events, state = step(events, state, rotate(b))
	events, state = step(events, state, approve)
	if current, err := verifyHistory(events, state.Account); err != nil || current.Inbox != b {
		t.Fatal("valid rotation history rejected", err)
	}
	reinstated, _ := step(events, state, rotate(a))
	if _, err := verifyHistory(reinstated, state.Account); err == nil {
		t.Fatal("retired incoming-share key was reinstated")
	}
	fresh, _ := step(events, state, rotate(encoded(32)))
	if current, err := verifyHistory(fresh, state.Account); err != nil || current.Epoch != 3 {
		t.Fatal("fresh rotation rejected", err)
	}
}
