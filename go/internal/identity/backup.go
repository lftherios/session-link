package identity

import (
	"context"
	"errors"
)

// Backup is best-effort after a successful share; a key remains durable locally
// even when the account vault is unavailable or this device is not approved.
func (c Client) Backup(ctx context.Context, account string) string {
	if account == "" {
		return "setup"
	}
	d, err := c.load(account)
	if err != nil {
		return "failed"
	}
	if d == nil || d.Root == "" {
		return "setup"
	}
	_, err = c.Handle(ctx, "sync", Input{})
	if err == ErrLocked {
		return "locked"
	}
	if err != nil {
		return "failed"
	}
	return "synced"
}

// HTTPStatus preserves authentication failures for the local sign-in UI.
func HTTPStatus(err error) int {
	var httpErr *httpError
	if errors.As(err, &httpErr) && (httpErr.status == 401 || httpErr.status == 403 || httpErr.status == 409 || httpErr.status == 429) {
		return httpErr.status
	}
	return 422
}
