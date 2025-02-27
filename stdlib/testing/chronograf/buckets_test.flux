package chronograf_test


import "testing"

inData = "
#datatype,string,long,string,string,string,string,long,dateTime:RFC3339,string,string
#group,false,false,true,false,false,false,false,false,true,true
#default,_result,,0389eade5af4b000,,,,,,,
,result,table,organizationID,name,id,retentionPolicy,retentionPeriod,_time,_field,_measurement
,,0,,A,0389eade5b34b000,,0,1970-01-01T00:00:00Z,a,aa
,,0,,B,042ed3f42d42e000,,0,1970-01-01T00:00:00Z,b,bb
"
outData = "
#datatype,string,long,string
#group,false,false,false
#default,_result,,
,result,table,_value
,,0,A
,,0,B
"
buckets_fn = (table=<-) => table
    |> rename(columns: {name: "_value"})
    |> keep(columns: ["_value"])

test buckets = () => ({input: testing.loadStorage(csv: inData), want: testing.loadMem(csv: outData), fn: buckets_fn})
