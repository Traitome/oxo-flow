process echo_step {
    tag "step_${idx}"
    // Publish each step's declared output into the launch directory so the
    // produced artifacts are directly comparable to the other two engines'
    // outputs (Nextflow otherwise keeps them only in per-task work dirs).
    publishDir './results', mode: 'copy'

    input:
    path input_file
    val idx

    output:
    path "step_${idx}_output.txt"

    script:
    """
    echo "${idx}" > "step_${idx}_output.txt"
    """
}
