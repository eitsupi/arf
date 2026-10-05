# Generate independently authored, minimal installed-help fixtures.
# Run: Rscript generate.R /path/to/help_resolution
args <- commandArgs(trailingOnly = TRUE)
stopifnot(length(args) == 1L)
dir.create(args[[1L]], recursive = TRUE, showWarnings = FALSE)
db <- new.env(parent = emptyenv())
for (topic in c("first-topic", "second-topic")) {
  source <- sprintf("\\name{%s}\n\\title{Fixture %s}\n\\description{A resolver fixture.}\n", topic, topic)
  db[[topic]] <- tools::parse_Rd(textConnection(source))
}
tools:::makeLazyLoadDB(db, file.path(args[[1L]], "resolverpkg"), compress = TRUE)
operators <- c("[", "[[", "%/%", "%in%", "/")
aliases <- c(
  mean = "first-topic", lm = "second-topic", shared = "first-topic",
  duplicate = "first-topic", duplicate = "second-topic",
  "first-topic" = "second-topic",
  setNames(rep("first-topic", length(operators)), operators)
)
saveRDS(aliases, file.path(args[[1L]], "aliases.rds"), compress = FALSE)
saveRDS(c(home = "first-topic"), file.path(args[[1L]], "source_aliases.rds"), compress = FALSE)
metadata <- data.frame(
  Name = c("first-topic", "second-topic"),
  File = c("first-topic.Rd", "second-topic.Rd"),
  Title = c("First fixture", "Second fixture"),
  Aliases = I(list(c("mean", "shared", operators), c("lm", "first-topic")))
)
saveRDS(metadata, file.path(args[[1L]], "Rd.rds"), compress = FALSE)
saveRDS(metadata[, c("Name", "Title", "Aliases")], file.path(args[[1L]], "Rd_without_keys.rds"), compress = FALSE)
