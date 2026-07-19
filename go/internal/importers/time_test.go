package importers

import "time"

func timeAgo(days int) time.Time { return time.Now().Add(-time.Duration(days) * 24 * time.Hour) }
