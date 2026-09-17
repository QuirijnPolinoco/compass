scale_values <- function(x) {
  (x - mean(x)) / sd(x)
}
