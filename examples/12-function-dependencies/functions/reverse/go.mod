// Your own module. A single-file `.go` function gets a go.mod generated for it;
// a directory brings its own, so you can `require` other modules and split the
// code across files. apiplant builds it with `go build -buildmode=c-shared` and
// copies the result to ../libreverse.so.
module reverse

go 1.21
