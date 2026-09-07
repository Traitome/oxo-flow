process echo_step {
    tag "step_${idx}"

    input:
    path input_file
    val idx

    output:
    path "step_${idx}_output.txt"

    """
    echo "${idx}" > "step_${idx}_output.txt"
    """
}
