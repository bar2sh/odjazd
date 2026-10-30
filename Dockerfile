# The odjazd web app in a small container:
#
#   docker build -t odjazd-web .
#   docker run -p 8080:8080 -v odjazd-data:/data odjazd-web
#
# On first start it downloads Warsaw's timetable (about 110 MB) into /data.

FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p odjazd-web

FROM debian:trixie-slim
RUN useradd --system odjazd && mkdir /data && chown odjazd /data
COPY --from=build /src/target/release/odjazd-web /usr/local/bin/
USER odjazd
ENV ODJAZD_HOST=0.0.0.0 \
    PORT=8080 \
    ODJAZD_DATA_DIR=/data
VOLUME /data
EXPOSE 8080
CMD ["odjazd-web"]
