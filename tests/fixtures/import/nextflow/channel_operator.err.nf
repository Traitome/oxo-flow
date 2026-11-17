// EXPECT-ERROR: unsupported expression
process COUNT {
    input: path x
    output: "${x}.count"
    script: "wc -l < ${x} > ${x}.count"
}
workflow { COUNT(Channel.fromPath("data/*.txt").map { it }) }
