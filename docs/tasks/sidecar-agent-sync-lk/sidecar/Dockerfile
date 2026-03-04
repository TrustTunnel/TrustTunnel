FROM golang:1.21-alpine as build
WORKDIR /app
COPY ./src .
RUN go mod init sidecar && go get github.com/gorilla/websocket@v1.5.3 && go build -o /bin/sidecar ./main.go

FROM alpine:3.18
COPY --from=build /bin/sidecar /usr/local/bin/sidecar
ENTRYPOINT ["/usr/local/bin/sidecar"]
