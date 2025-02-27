package universe_test


import "testing"

option now = () => 2030-01-01T00:00:00Z

inData = "
#datatype,string,long,dateTime:RFC3339,string,double,double,string
#group,false,false,true,true,false,false,true
#default,_result,,,,,,
,result,table,_time,_field,count,upperBound,_measurement
,,0,2018-05-22T19:53:00Z,x_duration_seconds,1,0.1,l
,,0,2018-05-22T19:53:00Z,x_duration_seconds,2,0.2,l
,,0,2018-05-22T19:53:00Z,x_duration_seconds,2,0.3,l
,,0,2018-05-22T19:53:00Z,x_duration_seconds,2,0.4,l
,,0,2018-05-22T19:53:00Z,x_duration_seconds,2,0.5,l
,,0,2018-05-22T19:53:00Z,x_duration_seconds,2,0.6,l
,,0,2018-05-22T19:53:00Z,x_duration_seconds,2,0.7,l
,,0,2018-05-22T19:53:00Z,x_duration_seconds,8,0.8,l
,,0,2018-05-22T19:53:00Z,x_duration_seconds,10,0.9,l
,,0,2018-05-22T19:53:00Z,x_duration_seconds,10,+Inf,l
,,1,2018-05-22T19:53:00Z,y_duration_seconds,0,-Inf,l
,,1,2018-05-22T19:53:00Z,y_duration_seconds,10,0.2,l
,,1,2018-05-22T19:53:00Z,y_duration_seconds,15,0.4,l
,,1,2018-05-22T19:53:00Z,y_duration_seconds,25,0.6,l
,,1,2018-05-22T19:53:00Z,y_duration_seconds,35,0.8,l
,,1,2018-05-22T19:53:00Z,y_duration_seconds,45,1,l
,,1,2018-05-22T19:53:00Z,y_duration_seconds,45,+Inf,l
"
outData = "
#datatype,string,long,dateTime:RFC3339,dateTime:RFC3339,dateTime:RFC3339,string,double,string
#group,false,false,true,true,true,true,false,true
#default,_result,,,,,,,
,result,table,_start,_stop,_time,_field,quant,_measurement
,,0,2018-05-22T19:53:00Z,2030-01-01T00:00:00Z,2018-05-22T19:53:00Z,x_duration_seconds,0.8500000000000001,l
,,1,2018-05-22T19:53:00Z,2030-01-01T00:00:00Z,2018-05-22T19:53:00Z,y_duration_seconds,0.91,l
"
t_histogram_quantile = (table=<-) => table
    |> range(start: 2018-05-22T19:53:00Z)
    |> histogramQuantile(quantile: 0.9, upperBoundColumn: "upperBound", countColumn: "count", valueColumn: "quant")

test _histogram_quantile = () => ({input: testing.loadStorage(csv: inData), want: testing.loadMem(csv: outData), fn: t_histogram_quantile})
