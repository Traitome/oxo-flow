#!/usr/bin/env nextflow
nextflow.enable.dsl=2

/*
 * oxo-flow 对比基准 — Nextflow 等效管线
 *
 * 与 benches/macro/suite.py 中 generate_hello(N) 生成的 oxo-flow
 * 管线在功能上等价：N 步顺序链，每步将输入拷贝到输出。
 *
 * 用法:
 *   nextflow run hello.nf --count 10
 */

params.count = 10

// 各步骤顺序执行，每步依赖上一步的输出
process step {
    tag "step_${task.index - 1}"

    input:
    path input_file

    output:
    path "step_${task.index - 1}_output.txt"

    exec """
    if [ -f "${input_file}" ]; then
        cp "${input_file}" "step_${task.index - 1}_output.txt"
    else
        echo "${task.index - 1}" > "step_${task.index - 1}_output.txt"
    fi
    """
}

workflow {
    // 第一个规则使用输入文件或创建初始文件
    step( file("input.txt") )

    // 后续 N-1 个规则
    for (int i = 1; i < params.count; i++) {
        step( file("step_${i - 1}_output.txt") )
    }
}
