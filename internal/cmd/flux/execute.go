package main

import (
	"context"
	"fmt"
	"os"

	"github.com/influxdata/flux"
	"github.com/influxdata/flux/csv"
	"github.com/influxdata/flux/execute"
	"github.com/influxdata/flux/lang"
	"github.com/influxdata/flux/memory"
	"github.com/influxdata/flux/runtime"
)

func executeE(ctx context.Context, script, format string) error {
	c := lang.FluxCompiler{
		Query: script,
	}
	prog, err := c.Compile(ctx, runtime.Default)
	if err != nil {
		return err
	}

	mem := &memory.Allocator{}
	q, err := prog.Start(ctx, mem)
	if err != nil {
		return err
	}

	results := flux.NewResultIteratorFromQuery(q)
	defer results.Release()

	if format == "cli" {
		for results.More() {
			res := results.Next()
			fmt.Println("Result:", res.Name())
			if err := res.Tables().Do(func(table flux.Table) error {
				_, err := execute.NewFormatter(table, nil).WriteTo(os.Stdout)
				return err
			}); err != nil {
				return err
			}
		}
	} else if format == "csv" {
		config := csv.DefaultEncoderConfig()
		encoder := csv.NewMultiResultEncoder(config)
		_, err := encoder.Encode(os.Stdout, results)
		if err != nil {
			return err
		}
	}
	results.Release()
	return results.Err()
}
