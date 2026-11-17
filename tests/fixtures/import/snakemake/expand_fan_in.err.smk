# EXPECT-ERROR: fan-in
# expand() fan-in: one constant output consumes many per-sample files.
# Mapping this to a wildcard input would create one rule instance per sample,
# all writing results/report.txt.
rule report:
    input:
        expand("quant/{sample}.sf", sample=["A", "B"])
    output:
        "results/report.txt"
    shell:
        "cat {input} > {output}"
