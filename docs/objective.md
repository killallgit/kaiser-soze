# Objective

Capture every request a coding agent sends to its model, and every streamed reply, through a
custom local proxy. Adapt that traffic into one common, stripped-down format for use by other
applications.

The proxy is the method because it is:

- **Fastest.** It sees each chunk as the model streams it, before the agent records anything.
- **Robust.** It never gets in the way. Bytes reach the agent untouched, the tap reads a copy,
  and a failure in the tap costs only tap data.
- **Future-proof.** Every agent talks to a model API over HTTPS, so one small `Adapter` per tool
  covers new tools without changing the core.

The common format keeps only what the model actually sees and says
([common-format.md](common-format.md)). Fields a single tool adds are left out unless the
format needs them.

How the format reaches other applications is **TBD**. Do not assume any transport, message
bus, database or store until the user decides.
