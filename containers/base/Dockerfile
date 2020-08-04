FROM registry.access.redhat.com/ubi8-minimal

RUN microdnf install -y java-11-openjdk-headless
RUN microdnf install -y stunnel

ADD mctunnel.conf /etc/