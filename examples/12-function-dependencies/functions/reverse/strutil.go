// A second file in the same module — the "extra code" a directory makes room
// for. No cgo here; it's plain Go the entry point calls.
package main

// reverse returns s with its runes in reverse order.
func reverse(s string) string {
	runes := []rune(s)
	for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
		runes[i], runes[j] = runes[j], runes[i]
	}
	return string(runes)
}
