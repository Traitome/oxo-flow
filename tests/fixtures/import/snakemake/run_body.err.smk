# EXPECT-ERROR: `run:` is not supported
rule custom:
    input:
        "data/{sample}.txt"
    output:
        "out/{sample}.txt"
    run:
        text = open(input[0]).read()
        open(output[0], "w").write(text)
