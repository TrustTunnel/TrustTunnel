FROM golang:1.22-alpine as build
WORKDIR /app

COPY go.mod ./
RUN go mod download

COPY ./src ./src
RUN go build -o /bin/sidecar ./src

FROM alpine:3.19
WORKDIR /work
COPY --from=build /bin/sidecar /usr/local/bin/sidecar
ENTRYPOINT ["/usr/local/bin/sidecar"]
