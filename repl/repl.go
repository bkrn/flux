// Package repl implements the read-eval-print-loop for the command line flux query console.
package repl

import (
	"context"
	"fmt"
	"io/ioutil"
	"os"
	"os/signal"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"syscall"

	"github.com/c-bata/go-prompt"
	"github.com/influxdata/flux"
	"github.com/influxdata/flux/execute"
	"github.com/influxdata/flux/internal/spec"
	"github.com/influxdata/flux/interpreter"
	"github.com/influxdata/flux/lang"
	"github.com/influxdata/flux/libflux/go/libflux"
	"github.com/influxdata/flux/memory"
	"github.com/influxdata/flux/runtime"
	"github.com/influxdata/flux/semantic"
	"github.com/influxdata/flux/values"
)

type REPL struct {
	ctx  context.Context
	deps flux.Dependencies

	scope    values.Scope
	itrp     *interpreter.Interpreter
	analyzer *libflux.Analyzer
	importer interpreter.Importer

	cancelMu   sync.Mutex
	cancelFunc context.CancelFunc
}

func New(ctx context.Context, deps flux.Dependencies) *REPL {
	scope := values.NewScope()
	importer := runtime.StdLib()
	for _, p := range runtime.PreludeList {
		pkg, err := importer.ImportPackageObject(p)
		if err != nil {
			panic(err)
		}
		pkg.Range(scope.Set)
	}
	return &REPL{
		ctx:      ctx,
		deps:     deps,
		scope:    scope,
		itrp:     interpreter.NewInterpreter(nil, &lang.ExecOptsConfig{}),
		analyzer: libflux.NewAnalyzer(),
		importer: importer,
	}
}

func (r *REPL) Run() {
	p := prompt.New(
		r.input,
		r.completer,
		prompt.OptionPrefix("> "),
		prompt.OptionTitle("flux"),
	)
	sigs := make(chan os.Signal, 1)
	signal.Notify(sigs, syscall.SIGINT)
	go func() {
		for range sigs {
			r.cancel()
		}
	}()
	p.Run()
}

func (r *REPL) cancel() {
	r.cancelMu.Lock()
	defer r.cancelMu.Unlock()
	if r.cancelFunc != nil {
		r.cancelFunc()
		r.cancelFunc = nil
	}
}

func (r *REPL) setCancel(cf context.CancelFunc) {
	r.cancelMu.Lock()
	defer r.cancelMu.Unlock()
	r.cancelFunc = cf
}
func (r *REPL) clearCancel() {
	r.setCancel(nil)
}

func (r *REPL) completer(d prompt.Document) []prompt.Suggest {
	names := make([]string, 0, r.scope.Size())
	r.scope.Range(func(k string, v values.Value) {
		names = append(names, k)
	})
	sort.Strings(names)

	s := make([]prompt.Suggest, 0, len(names))
	for _, n := range names {
		if n == "_" || !strings.HasPrefix(n, "_") {
			s = append(s, prompt.Suggest{Text: n})
		}
	}
	if d.Text == "" || strings.HasPrefix(d.Text, "@") {
		root := "./" + strings.TrimPrefix(d.Text, "@")
		fluxFiles, err := getFluxFiles(root)
		if err == nil {
			for _, fName := range fluxFiles {
				s = append(s, prompt.Suggest{Text: "@" + fName})
			}
		}
		dirs, err := getDirs(root)
		if err == nil {
			for _, fName := range dirs {
				s = append(s, prompt.Suggest{Text: "@" + fName + string(os.PathSeparator)})
			}
		}
	}

	return prompt.FilterHasPrefix(s, d.GetWordBeforeCursor(), true)
}

func (r *REPL) Input(t string) error {
	return r.executeLine(t)
}

// input processes a line of input and prints the result.
func (r *REPL) input(t string) {
	if err := r.executeLine(t); err != nil {
		fmt.Println("Error:", err)
	}
}

func (r *REPL) Eval(t string) ([]interpreter.SideEffect, error) {
	if t == "" {
		return nil, nil
	}

	if t[0] == '@' {
		q, err := LoadQuery(t)
		if err != nil {
			return nil, err
		}
		t = q
	}

	pkg, err := r.analyzeLine(t)
	if err != nil {
		return nil, err
	}

	deps := execute.DefaultExecutionDependencies()
	r.ctx = deps.Inject(r.ctx)

	return r.itrp.Eval(r.ctx, pkg, r.scope, r.importer)
}

// executeLine processes a line of input.
// If the input evaluates to a valid value, that value is returned.
func (r *REPL) executeLine(t string) error {
	ses, err := r.Eval(t)
	if err != nil {
		return err
	}

	for _, se := range ses {
		if _, ok := se.Node.(*semantic.ExpressionStatement); ok {
			if t, ok := se.Value.(*flux.TableObject); ok {
				now, ok := r.scope.Lookup("now")
				if !ok {
					return fmt.Errorf("now option not set")
				}
				ctx := r.deps.Inject(context.TODO())
				nowTime, err := now.Function().Call(ctx, nil)
				if err != nil {
					return err
				}
				s, err := spec.FromTableObject(r.ctx, t, nowTime.Time().Time())
				if err != nil {
					return err
				}
				if err := r.doQuery(r.ctx, s, r.deps); err != nil {
					return err
				}
			} else {
				values.Display(os.Stdout, se.Value)
				fmt.Println()
			}
		}
	}
	return nil
}

func (r *REPL) analyzeLine(t string) (*semantic.Package, error) {
	pkg, err := r.analyzer.Analyze(libflux.ParseString(t))
	if err != nil {
		return nil, err
	}

	bs, err := pkg.MarshalFB()
	if err != nil {
		return nil, err
	}
	return semantic.DeserializeFromFlatBuffer(bs)
}

func (r *REPL) doQuery(ctx context.Context, spec *flux.Spec, deps flux.Dependencies) error {
	// Setup cancel context
	ctx, cancelFunc := context.WithCancel(ctx)
	r.setCancel(cancelFunc)
	defer cancelFunc()
	defer r.clearCancel()

	c := Compiler{
		Spec: spec,
	}

	program, err := c.Compile(ctx, runtime.Default)
	if err != nil {
		return err
	}
	alloc := &memory.Allocator{}

	qry, err := program.Start(deps.Inject(ctx), alloc)
	if err != nil {
		return err
	}
	defer qry.Done()

	for result := range qry.Results() {
		tables := result.Tables()
		fmt.Println("Result:", result.Name())
		if err := tables.Do(func(tbl flux.Table) error {
			_, err := execute.NewFormatter(tbl, nil).WriteTo(os.Stdout)
			return err
		}); err != nil {
			return err
		}
	}
	qry.Done()
	return qry.Err()
}

func getFluxFiles(path string) ([]string, error) {
	return filepath.Glob(path + "*.flux")
}

func getDirs(path string) ([]string, error) {
	dir := filepath.Dir(path)
	files, err := ioutil.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	dirs := make([]string, 0, len(files))
	for _, f := range files {
		if f.IsDir() {
			dirs = append(dirs, filepath.Join(dir, f.Name()))
		}
	}
	return dirs, nil
}

// LoadQuery returns the Flux query q, except for two special cases:
// if q is exactly "-", the query will be read from stdin;
// and if the first character of q is "@",
// the @ prefix is removed and the contents of the file specified by the rest of q are returned.
func LoadQuery(q string) (string, error) {
	if q == "-" {
		data, err := ioutil.ReadAll(os.Stdin)
		if err != nil {
			return "", err
		}
		return string(data), nil
	}

	if len(q) > 0 && q[0] == '@' {
		data, err := ioutil.ReadFile(q[1:])
		if err != nil {
			return "", err
		}

		return string(data), nil
	}

	return q, nil
}
